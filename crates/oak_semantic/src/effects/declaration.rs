use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::RArgument;
use aether_syntax::RCall;
use biome_rowan::AstSeparatedList;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use crate::effects::CallContext;
use crate::effects::Effects;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;
use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;

/// A function's declared effects. Owned and cheap, with no lifetime, so the same
/// value serves both the registry (parsed once into a `LazyLock`) and local
/// `declare()` (built in the scope that resolves it). One [`resolve`] interprets
/// it against a call to yield the existing [`Effects`].
#[derive(Debug, Clone)]
pub struct Declaration {
    /// The callee's formals, in signature order. May be a leading prefix of the
    /// signature: it lists enough formals to cover every argument named by
    /// `arguments`. `ArgumentRef` indexes into this list, and only formals named
    /// by `arguments` are matched against a call.
    pub formals: Vec<FormalDef>,
    /// Arg-centric effects: how individual arguments evaluate (`x = Quote`,
    /// `x = Nse(...)`).
    pub arguments: Vec<ArgumentEffect>,
    /// Effect-centric effects: environment effects and where each reads its
    /// operands.
    pub env: Vec<EnvironmentEffect>,
}

impl Declaration {
    /// Start a declaration from the callee's leading formals, in signature
    /// order. Every formal defaults to no static default.
    pub fn new(formals: &[&str]) -> Self {
        Declaration {
            formals: formals
                .iter()
                .map(|name| FormalDef {
                    name: name.to_string(),
                    default: None,
                })
                .collect(),
            arguments: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Add an `Nse` effect on the formal at `arg` (an index into `formals`).
    /// `scope` is an env operand resolved against the call.
    pub fn nse(mut self, arg: usize, scope: DeclExpr, timing: NseTiming) -> Self {
        self.arguments.push(ArgumentEffect {
            arg: ArgumentRef(arg),
            mode: EvalMode::Nse { scope, timing },
        });
        self
    }

    /// Add a `Quote` effect on the formal at `arg` (an index into `formals`).
    pub fn quote(mut self, arg: usize) -> Self {
        self.arguments.push(ArgumentEffect {
            arg: ArgumentRef(arg),
            mode: EvalMode::Quote,
        });
        self
    }

    /// Give the formal at `arg` a static default, used when a call omits it.
    pub fn formal_default(mut self, arg: usize, default: StaticValue) -> Self {
        if let Some(formal) = self.formals.get_mut(arg) {
            formal.default = Some(default);
        }
        self
    }

    /// Add an `Attach` effect reading its package name from `package`.
    pub fn attach(mut self, package: DeclExpr) -> Self {
        self.env.push(EnvironmentEffect::Attach { package });
        self
    }

    /// Add a `Source` effect reading its path from `path`. `envir` resolves to
    /// the target scope for the sourced names, or drops the effect when it can't.
    pub fn source(mut self, path: DeclExpr, envir: DeclExpr) -> Self {
        self.env.push(EnvironmentEffect::Source { path, envir });
        self
    }
}

/// One formal in a [`Declaration`]'s signature.
#[derive(Debug, Clone)]
pub struct FormalDef {
    pub name: String,
    /// The formal's static default, consulted when the argument is absent.
    pub default: Option<StaticValue>,
}

/// A statically known literal an absent argument falls back to. It grows as
/// declarations need other literal types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticValue {
    Bool(bool),
    /// An env-capture op default (`envir = parent.frame()`), so an absent env
    /// argument resolves from it.
    Env(EnvOp),
}

/// An arg-centric effect: one argument and how it evaluates.
#[derive(Debug, Clone)]
pub struct ArgumentEffect {
    /// Index into [`Declaration::formals`].
    pub arg: ArgumentRef,
    pub mode: EvalMode,
}

/// How an argument's own sub-expressions are treated.
#[derive(Debug, Clone)]
pub enum EvalMode {
    /// Captured unevaluated. bquote-style unquote holes stay a custom handler.
    Quote,
    /// Quote plus eval in a controlled scope, fused. `scope` is an env operand
    /// resolved against the call, the same `.()`-hole grammar `Source.envir`
    /// uses, so a function's scope can be read from an argument.
    Nse { scope: DeclExpr, timing: NseTiming },
}

/// An effect-centric effect: what a call does to the surrounding environment.
#[derive(Debug, Clone)]
pub enum EnvironmentEffect {
    /// Read and evaluate another file, injecting its top-level names. `envir`
    /// resolves to an `NseScope` saying where those names land, and
    /// `source(local=)` maps onto it through the stub's if/else (`TRUE` ->
    /// `Current`, `FALSE` -> `Global`). An `envir` that fails to resolve (a
    /// non-static `local`, an explicit environment) drops the effect, which
    /// reproduces the old guard bail without a bespoke slot.
    Source { path: DeclExpr, envir: DeclExpr },
    /// Attach a package.
    Attach { package: DeclExpr },
}

/// An operand of an environment effect: a `.()` hole, or a branch selecting
/// between holes on a static-bool condition.
#[derive(Debug, Clone)]
pub enum DeclExpr {
    Hole(RExpr),
    If {
        cond: RExpr,
        then: Box<DeclExpr>,
        els: Box<DeclExpr>,
    },
}

impl DeclExpr {
    /// A `.()` hole that forces the argument at `arg` to its value.
    pub fn eval(arg: usize) -> Self {
        DeclExpr::Hole(RExpr::Eval(ArgumentRef(arg)))
    }

    /// A `.()` hole that captures the argument at `arg` unevaluated.
    pub fn substitute(arg: usize) -> Self {
        DeclExpr::Hole(RExpr::Substitute(ArgumentRef(arg)))
    }
}

/// The bounded R interpreted inside a `.()` hole.
#[derive(Debug, Clone, Copy)]
pub enum RExpr {
    /// `.(x)`: force the argument to its value. A live use.
    Eval(ArgumentRef),
    /// `.(substitute(x))`: capture its expression. Inert, implies `x` is quoted.
    Substitute(ArgumentRef),
    /// `.(parent.frame())`, `.(globalenv())`: an env-capture op. Denotes a
    /// scope, forces no argument, so it's never a live use.
    Env(EnvOp),
}

/// An environment-capture op as written in a stub. Deliberately not an
/// `NseScope`: the same op denotes different scopes in different frames. An op
/// written in the stub is callee-relative, so `parent.frame()` there is the
/// call site; an op read from an explicit argument names a frame we can't
/// interpret. `resolve_env_op` maps a stub-position op to a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvOp {
    /// `parent.frame()`, rlang `caller_env()`: the caller's scope.
    ParentFrame,
    /// `globalenv()`, rlang `global_env()`: the global scope.
    GlobalEnv,
    /// `environment()`, rlang `current_env()`: the callee's own frame, not a
    /// scope user code resolves against.
    Environment,
    /// `new.env(parent = ...)`: a fresh scope.
    NewEnv { parent: EnvParent },
}

/// `new.env`'s parent, one level. Bare `new.env()` is `new.env(parent =
/// environment())` at the point it's written, so it parses to `Environment`
/// ("here"). `Unknown` covers out-of-set parents (`baseenv()`, a variable). The
/// first cut resolves every fresh env to `Nested` regardless of parent, keeping
/// the op for a future detached-scope refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvParent {
    ParentFrame,
    Environment,
    GlobalEnv,
    Unknown,
}

/// An index into the enclosing [`Declaration::formals`]. Name and position both
/// live in [`FormalDef`], so this is just the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentRef(pub usize);

/// One file a `source()` call reads, plus where its top-level names land.
#[derive(Debug, Clone)]
pub struct SourcedPath {
    pub path: String,
    /// The target scope from env capture. `Current` (call-site scope) or
    /// `Global`. `Nested` never arises for source.
    pub scope: NseScope,
}

/// Interpret a declaration against a call, producing the owned [`Effects`] the
/// builder consumes.
///
/// The call is matched against the declaration's `formals` once with
/// [`match_signature`]. The arg-centric axis (`arguments`) maps each matched
/// formal to its declared [`EvalMode`]. The effect-centric axis (`env`)
/// interprets each [`EnvironmentEffect`] against the same match to yield the
/// attach package / source path, and folds the liveness of any `Substitute`
/// operand it consulted back into `arguments` (a captured argument is inert, so
/// its slot becomes `Quote` and the walk stops treating it as a use).
pub fn resolve(declaration: &Declaration, call: &RCall, ctx: &CallContext) -> Option<Effects> {
    let matched = match_signature(call, &declaration.formals);
    let values = argument_values(call);

    let resolution = EnvResolution {
        declaration,
        matched: &matched,
        values: &values,
        ctx,
    };

    let mut arguments = resolution.resolve_arguments();
    let mut attach = None;
    let mut source = None;

    for effect in &declaration.env {
        resolution.resolve_effect(effect, &mut arguments, &mut attach, &mut source);
    }

    Some(Effects {
        arguments,
        attach,
        source,
        ..Effects::default()
    })
}

/// One call matched against one declaration, the shared context for interpreting
/// every environment effect: the declaration (for formal defaults), the
/// formal-to-argument match, the argument value expressions, and the resolution
/// context.
struct EnvResolution<'a> {
    declaration: &'a Declaration,
    matched: &'a [Option<usize>],
    values: &'a [Option<AnyRExpression>],
    ctx: &'a CallContext,
}

/// Which kind of string an environment slot expects, which decides how a
/// `Substitute` capture is coerced.
enum StringSlot {
    /// A package name: an `Eval` result is a static string, a `Substitute`
    /// capture is the symbol or string as written.
    PackageName,
    /// A file path: an `Eval` result is a static string, a `Substitute` capture
    /// doesn't name a path and so is unresolved.
    Path,
}

impl EnvResolution<'_> {
    /// Map each call argument to its declared [`EvalMode`], in call order. `None`
    /// when the declaration names no arguments.
    fn resolve_arguments(&self) -> Option<ResolvedArgumentEffects> {
        if self.declaration.arguments.is_empty() {
            return None;
        }

        Some(
            self.matched
                .iter()
                .map(|formal| {
                    formal.and_then(|idx| {
                        declared_mode(self.declaration, idx).map(|mode| self.resolve_mode(mode))
                    })
                })
                .collect(),
        )
    }

    /// Interpret one declared [`EvalMode`] against the call. `Nse` resolves its
    /// scope operand the same way an environment effect resolves `envir`.
    fn resolve_mode(&self, mode: &EvalMode) -> ResolvedArgumentEffect {
        match mode {
            EvalMode::Nse { scope, timing } => match self.resolve_env_expr(scope) {
                Some(scope) => ResolvedArgumentEffect::Nse {
                    scope,
                    timing: *timing,
                },
                // An `Nse` argument is always captured. When we can't resolve
                // its scope (an explicit env argument), we still know it's
                // quoted, we just don't know where it evaluates. Suppressing it
                // as `Quote` is safer than treating it as a plain use in the
                // wrong scope.
                None => ResolvedArgumentEffect::Quote { holes: Vec::new() },
            },
            EvalMode::Quote => ResolvedArgumentEffect::Quote { holes: Vec::new() },
        }
    }

    /// Interpret one environment effect, writing its result into `attach`/`source`
    /// and folding consulted `Substitute` operands into `arguments`. An operand,
    /// condition, or guard that fails to resolve drops this one effect (fold
    /// nothing) and leaves the others untouched.
    fn resolve_effect(
        &self,
        effect: &EnvironmentEffect,
        arguments: &mut Option<ResolvedArgumentEffects>,
        attach: &mut Option<String>,
        source: &mut Option<Vec<SourcedPath>>,
    ) {
        let mut folds = Vec::new();

        match effect {
            EnvironmentEffect::Source { path, envir } => {
                // `envir` resolves to the target scope for the sourced names. A
                // non-static `local` (or any env we can't map to a scope) leaves
                // it unresolved and drops the effect, the same bail the old guard
                // gave a non-static `local =`.
                let Some(scope) = self.resolve_env_expr(envir) else {
                    return;
                };
                let Some(path) = self.resolve_string_expr(path, StringSlot::Path, &mut folds)
                else {
                    return;
                };
                source
                    .get_or_insert_with(Vec::new)
                    .push(SourcedPath { path, scope });
            },
            EnvironmentEffect::Attach { package } => {
                let Some(package) =
                    self.resolve_string_expr(package, StringSlot::PackageName, &mut folds)
                else {
                    return;
                };
                *attach = Some(package);
            },
        }

        apply_folds(arguments, &folds, self.matched.len());
    }

    /// Resolve a [`DeclExpr`] in a string slot, recording consulted `Substitute`
    /// operands into `folds`. Only the chosen branch of an `If` contributes folds.
    fn resolve_string_expr(
        &self,
        expr: &DeclExpr,
        slot: StringSlot,
        folds: &mut Vec<usize>,
    ) -> Option<String> {
        match expr {
            DeclExpr::Hole(operand) => self.resolve_string_operand(operand, slot, folds),
            DeclExpr::If { cond, then, els } => {
                let cond = self.resolve_bool_operand(cond)?;
                let branch = if cond { then } else { els };
                self.resolve_string_expr(branch, slot, folds)
            },
        }
    }

    /// Resolve a single `.()` operand in a string slot. An `Eval` forces the
    /// argument to a static string; a `Substitute` captures it, records its slot
    /// for folding, and coerces the capture per `slot`.
    fn resolve_string_operand(
        &self,
        operand: &RExpr,
        slot: StringSlot,
        folds: &mut Vec<usize>,
    ) -> Option<String> {
        match operand {
            RExpr::Eval(ArgumentRef(arg)) => match self.formal_binding(*arg) {
                Some((_, value)) => self.ctx.resolve_static_string(value),
                // A string slot needs a string default, but `StaticValue` only
                // models bools, so an absent argument has no default to fall back
                // on.
                None => None,
            },
            RExpr::Substitute(ArgumentRef(arg)) => {
                let (pos, value) = self.formal_binding(*arg)?;
                folds.push(pos);
                match slot {
                    StringSlot::PackageName => self.ctx.resolve_quoted_symbol_or_string(value),
                    StringSlot::Path => None,
                }
            },
            // An env-capture op denotes a scope, not a string.
            RExpr::Env(_) => None,
        }
    }

    /// Resolve an operand expected to be a static bool (an `If` condition). Only
    /// an `Eval` can be a bool; a `Substitute` captures an expression and an
    /// `Env` op denotes a scope, neither of which is ever a bool.
    fn resolve_bool_operand(&self, operand: &RExpr) -> Option<bool> {
        match operand {
            RExpr::Eval(ArgumentRef(arg)) => match self.formal_binding(*arg) {
                Some((_, value)) => self.ctx.resolve_static_bool(value),
                None => static_bool(
                    self.declaration
                        .formals
                        .get(*arg)
                        .and_then(|formal| formal.default),
                ),
            },
            RExpr::Substitute(_) | RExpr::Env(_) => None,
        }
    }

    /// Resolve an env-expr to the target scope its capture denotes. An `If`
    /// selects a branch on a static bool, exactly how the old guard bail falls
    /// out. A non-static condition leaves it unresolved (`None`), which drops the
    /// source.
    fn resolve_env_expr(&self, expr: &DeclExpr) -> Option<NseScope> {
        match expr {
            DeclExpr::Hole(RExpr::Env(op)) => resolve_env_op(*op),
            DeclExpr::Hole(RExpr::Eval(ArgumentRef(arg))) => {
                // Reading an env from a `.()` hole. An explicit argument isn't
                // interpreted in this first cut, so it drops. Only an absent
                // argument reads the formal's env-op default. This is the
                // asymmetry with `resolve_bool_operand`, which does read an
                // explicit bool value: bools aren't frame-relative, so the
                // written value means the same thing everywhere; environments
                // are frame-relative, so a user-supplied env can't be mapped to
                // a scope here.
                if self.formal_binding(*arg).is_some() {
                    return None;
                }
                let default = self
                    .declaration
                    .formals
                    .get(*arg)
                    .and_then(|formal| formal.default);
                static_env(default).and_then(resolve_env_op)
            },
            // A captured expression (`.(substitute(x))`) never denotes a scope.
            DeclExpr::Hole(RExpr::Substitute(_)) => None,
            DeclExpr::If { cond, then, els } => {
                let cond = self.resolve_bool_operand(cond)?;
                let branch = if cond { then } else { els };
                self.resolve_env_expr(branch)
            },
        }
    }

    /// Find where the formal at `formal` was bound in the call: its call-argument
    /// position and value expression, or `None` when the argument is absent.
    fn formal_binding(&self, formal: usize) -> Option<(usize, &AnyRExpression)> {
        let pos = self
            .matched
            .iter()
            .position(|bound| *bound == Some(formal))?;
        let value = self.values.get(pos)?.as_ref()?;
        Some((pos, value))
    }
}

/// Mark each consulted `Substitute` operand's call slot as `Quote` (inert), so
/// the walk skips it. Creates the arguments vector if an arg-centric pass didn't
/// already, keeping it aligned 1:1 with the call. Never overwrites a slot an
/// [`ArgumentEffect`] already set.
fn apply_folds(arguments: &mut Option<ResolvedArgumentEffects>, folds: &[usize], arg_count: usize) {
    if folds.is_empty() {
        return;
    }
    let arguments = arguments.get_or_insert_with(|| vec![None; arg_count]);
    for &pos in folds {
        if arguments[pos].is_none() {
            arguments[pos] = Some(ResolvedArgumentEffect::Quote { holes: Vec::new() });
        }
    }
}

/// Read a static default as a bool, or `None` when there is no default or it
/// isn't a bool. The exhaustive match forces a decision here when `StaticValue`
/// grows a variant.
fn static_bool(value: Option<StaticValue>) -> Option<bool> {
    match value? {
        StaticValue::Bool(default) => Some(default),
        StaticValue::Env(_) => None,
    }
}

/// Read a static default as an env-capture op, or `None` when there is no
/// default or it isn't an env op. Mirrors [`static_bool`]; the exhaustive match
/// forces a decision here when `StaticValue` grows a variant.
fn static_env(value: Option<StaticValue>) -> Option<EnvOp> {
    match value? {
        StaticValue::Env(op) => Some(op),
        StaticValue::Bool(_) => None,
    }
}

/// Map a stub-position env-capture op to the scope it denotes in the callee's
/// frame. `parent.frame()` is the call site (`Current`), `globalenv()` is
/// `Global`, a fresh `new.env()` is a `Nested` scope. `environment()` is the
/// callee's own frame, not a scope user code resolves against, so it drops.
fn resolve_env_op(op: EnvOp) -> Option<NseScope> {
    match op {
        EnvOp::ParentFrame => Some(NseScope::Current),
        EnvOp::GlobalEnv => Some(NseScope::Global),
        // Callee's own frame: not a scope user code resolves against. Drops.
        EnvOp::Environment => None,
        // A fresh scope, regardless of parent in the first cut.
        EnvOp::NewEnv { .. } => Some(NseScope::Nested),
    }
}

/// The value expression of each call argument, in order, aligned with
/// [`match_signature`]'s output.
pub(crate) fn argument_values(call: &RCall) -> Vec<Option<AnyRExpression>> {
    let Ok(args) = call.arguments() else {
        return Vec::new();
    };
    args.items()
        .iter()
        .map(|item| item.ok().and_then(|arg| arg.value()))
        .collect()
}

/// The [`EvalMode`] declared for the formal at `idx`, if any. A matched formal
/// with no declared effect resolves its call slot to plain evaluation (`None`).
fn declared_mode(declaration: &Declaration, idx: usize) -> Option<&EvalMode> {
    declaration
        .arguments
        .iter()
        .find(|effect| effect.arg.0 == idx)
        .map(|effect| &effect.mode)
}

/// Match a call's arguments against a callee's `formals`, applying R's matching
/// rules restricted to what we model. Returns one entry per call argument in
/// order: `Some(index into formals)` for the formal it binds, or `None` for an
/// argument that binds no listed formal (an unknown name, or a positional that
/// falls through to `...`).
///
/// Exact named matching runs first. A named argument binds the formal with that
/// exact name and consumes it. No partial matching, by choice: R would prefix-
/// match but we don't. A named argument matching no listed formal binds nothing.
/// That is correct even though it looks lossy, because `formals` may be only a
/// leading prefix of the true signature. The unknown name may be a real formal we
/// don't list, and R would consume that formal by name, leaving the listed
/// formals free for positional filling. Binding nothing reproduces that, so a
/// prefix suffices.
///
/// Positional filling runs second over the still-unnamed arguments. Each fills
/// the next unconsumed formal in signature order, not the formal at the same raw
/// call position, so a leading named argument no longer shifts the count. `...`
/// stops positional filling. It and every later unnamed argument bind to dots
/// (`None`), and formals after `...` are reachable only by name, which the first
/// pass already handled.
pub(crate) fn match_signature(call: &RCall, formals: &[FormalDef]) -> Vec<Option<usize>> {
    let Ok(args) = call.arguments() else {
        return Vec::new();
    };
    let items = args.items();

    let arg_count = items.iter().count();
    let mut matched: Vec<Option<usize>> = vec![None; arg_count];
    let mut consumed = vec![false; formals.len()];

    // Exact named matching. `...` is never matched by name.
    for (i, item) in items.iter().enumerate() {
        let Ok(arg) = item else { continue };
        let Some(name) = argument_name(&arg) else {
            continue;
        };
        let bound = formals
            .iter()
            .enumerate()
            .find(|(idx, formal)| !consumed[*idx] && formal.name != "..." && formal.name == name)
            .map(|(idx, _)| idx);
        if let Some(idx) = bound {
            consumed[idx] = true;
            matched[i] = Some(idx);
        }
    }

    // Positional filling over the still-unnamed arguments, stopping at `...`.
    let mut cursor = 0;
    'positional: for (i, item) in items.iter().enumerate() {
        let Ok(arg) = item else { continue };
        if arg.name_clause().is_some() {
            continue;
        }
        let bound = loop {
            let Some(formal) = formals.get(cursor) else {
                break 'positional;
            };
            if formal.name == "..." {
                break 'positional;
            }
            if !consumed[cursor] {
                break cursor;
            }
            cursor += 1;
        };
        matched[i] = Some(bound);
        consumed[bound] = true;
        cursor += 1;
    }

    matched
}

/// The name a call argument is bound by, or `None` when it's positional. Names
/// may be identifiers or strings (`f("x" = 1)`), matching R.
pub(crate) fn argument_name(arg: &RArgument) -> Option<String> {
    let clause = arg.name_clause()?;
    match clause.name().ok()? {
        AnyRArgumentName::RIdentifier(ident) => Some(ident.name_text()),
        AnyRArgumentName::RStringValue(s) => s.string_text(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use aether_parser::parse;
    use aether_parser::RParserOptions;
    use biome_rowan::AstNode;
    use biome_rowan::WalkEvent;

    use super::*;

    /// Parse `source` and return its first call.
    fn first_call(source: &str) -> RCall {
        let parsed = parse(source, RParserOptions::default());
        assert!(!parsed.has_error());
        parsed
            .tree()
            .syntax()
            .preorder()
            .find_map(|event| match event {
                WalkEvent::Enter(node) => RCall::cast(node),
                WalkEvent::Leave(_) => None,
            })
            .unwrap()
    }

    /// A bare `Nse()` scope: `new.env(parent = parent.frame())`, which resolves
    /// to `Nested`.
    fn nested_scope() -> DeclExpr {
        DeclExpr::Hole(RExpr::Env(EnvOp::NewEnv {
            parent: EnvParent::ParentFrame,
        }))
    }

    /// A call-site scope: `parent.frame()`, which resolves to `Current`.
    fn current_scope() -> DeclExpr {
        DeclExpr::Hole(RExpr::Env(EnvOp::ParentFrame))
    }

    #[test]
    fn resolves_positional_nse_argument() {
        // `code` sits at positional index 1, so the block resolves to its Nse
        // effect and the leading `desc` stays plain.
        let call = first_call("test_that(\"d\", { x })");
        let declaration =
            Declaration::new(&["desc", "code"]).nse(1, nested_scope(), NseTiming::Eager);
        let effects = resolve(&declaration, &call, &CallContext::new()).unwrap();

        let arguments = effects.arguments.unwrap();
        assert_eq!(arguments.len(), 2);
        assert!(arguments[0].is_none());
        assert!(matches!(
            arguments[1],
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Nested,
                timing: NseTiming::Eager,
            })
        ));
    }

    #[test]
    fn nse_scope_operand_resolves_current() {
        // A scope operand written directly as `parent.frame()` resolves to the
        // call-site scope, `Current`.
        let call = first_call("f(foo)");
        let declaration = Declaration::new(&["expr"]).nse(0, current_scope(), NseTiming::Eager);
        let effects = resolve(&declaration, &call, &CallContext::new()).unwrap();

        assert!(matches!(
            effects.arguments.unwrap()[0],
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Eager,
            })
        ));
    }

    #[test]
    fn nse_scope_reads_formal_env_default() {
        // `expr = Nse(.(envir))` with `envir` defaulting to `parent.frame()`.
        // With `envir` absent, the scope reads that default and resolves to
        // `Current`.
        let call = first_call("f(foo)");
        let declaration = Declaration::new(&["expr", "envir"])
            .nse(0, DeclExpr::eval(1), NseTiming::Eager)
            .formal_default(1, StaticValue::Env(EnvOp::ParentFrame));
        let effects = resolve(&declaration, &call, &CallContext::new()).unwrap();

        let arguments = effects.arguments.unwrap();
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Eager,
            })
        ));
    }

    #[test]
    fn nse_explicit_env_argument_degrades_to_quote() {
        // The same declaration, but `envir` is supplied explicitly. An explicit
        // env argument isn't interpreted, so the scope drops and the `Nse`
        // degrades to a `Quote` (still captured, no scope pushed).
        let call = first_call("f(foo, e)");
        let declaration = Declaration::new(&["expr", "envir"])
            .nse(0, DeclExpr::eval(1), NseTiming::Eager)
            .formal_default(1, StaticValue::Env(EnvOp::ParentFrame));
        let effects = resolve(&declaration, &call, &CallContext::new()).unwrap();

        let arguments = effects.arguments.unwrap();
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Quote { .. })
        ));
    }

    #[test]
    fn resolves_quote_argument() {
        let call = first_call("quote(x + y)");
        let declaration = Declaration::new(&["expr"]).quote(0);
        let effects = resolve(&declaration, &call, &CallContext::new()).unwrap();

        let arguments = effects.arguments.unwrap();
        assert_eq!(arguments.len(), 1);
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Quote { .. })
        ));
    }

    #[test]
    fn matches_named_argument_out_of_order() {
        // A named `code =` binds by name regardless of call position, so the
        // effect lands on the first call argument here.
        let call = first_call("test_that(code = { x }, desc = \"d\")");
        let declaration =
            Declaration::new(&["desc", "code"]).nse(1, nested_scope(), NseTiming::Eager);
        let effects = resolve(&declaration, &call, &CallContext::new()).unwrap();

        let arguments = effects.arguments.unwrap();
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Nse { .. })
        ));
        assert!(arguments[1].is_none());
    }

    #[test]
    fn no_declared_arguments_yields_no_argument_effects() {
        let call = first_call("f(x)");
        let declaration = Declaration::new(&["x"]);
        let effects = resolve(&declaration, &call, &CallContext::new()).unwrap();
        assert!(effects.arguments.is_none());
    }

    /// Match `source` against `Declaration::new(names)`'s formals.
    fn match_names(source: &str, names: &[&str]) -> Vec<Option<usize>> {
        let call = first_call(source);
        let formals = Declaration::new(names).formals;
        match_signature(&call, &formals)
    }

    #[test]
    fn match_signature_pure_positional() {
        assert_eq!(match_names("f(1, 2)", &["a", "b"]), vec![Some(0), Some(1)]);
    }

    #[test]
    fn match_signature_named_then_positional_fills_remaining() {
        // `b =` consumes formal 1 by name, so the positional `2` fills the next
        // remaining formal in signature order (`a`), not the formal at its raw
        // call position.
        assert_eq!(match_names("f(b = 1, 2)", &["a", "b"]), vec![
            Some(1),
            Some(0)
        ]);
    }

    #[test]
    fn match_signature_block_first_fills_after_named() {
        // The legacy FIXME case. `desc =` is consumed by name, the block fills
        // the first remaining formal (`code`).
        assert_eq!(
            match_names("test_that({ x }, desc = \"d\")", &["desc", "code"]),
            vec![Some(1), Some(0)]
        );
    }

    #[test]
    fn match_signature_unknown_name_binds_nothing() {
        // `zzz` matches no listed formal, so it consumes nothing and the
        // positional `2` still fills the first formal.
        assert_eq!(match_names("f(zzz = 1, 2)", &["a", "b"]), vec![
            None,
            Some(0)
        ]);
    }

    #[test]
    fn match_signature_positional_skips_named_consumed_formal() {
        // `a =` consumed formal 0 by name, so the positional `2` must skip it and
        // fill formal 1 rather than rebinding formal 0.
        assert_eq!(match_names("f(a = 1, 2)", &["a", "b"]), vec![
            Some(0),
            Some(1)
        ]);
    }

    #[test]
    fn match_signature_dots_stop_positional_but_name_reaches_past() {
        // Formal `z` sits after `...`, reachable only by name. `1` fills `x`; the
        // trailing `3` hits `...` and binds to dots (`None`).
        assert_eq!(match_names("f(1, z = 2, 3)", &["x", "...", "z"]), vec![
            Some(0),
            Some(2),
            None
        ]);
    }

    /// An attach declaration shaped like `library`: `if (.(character.only))
    /// .(package) else .(substitute(package))`, with `character.only` at index 1.
    fn attach_declaration() -> Declaration {
        Declaration::new(&["package", "character.only"])
            .formal_default(1, StaticValue::Bool(false))
            .attach(DeclExpr::If {
                cond: RExpr::Eval(ArgumentRef(1)),
                then: Box::new(DeclExpr::eval(0)),
                els: Box::new(DeclExpr::substitute(0)),
            })
    }

    #[test]
    fn if_true_takes_eval_branch() {
        // `character.only = TRUE` picks the `then` branch, which forces the
        // package argument to a static string.
        let call = first_call("library(\"dplyr\", character.only = TRUE)");
        let effects = resolve(&attach_declaration(), &call, &CallContext::new()).unwrap();
        assert_eq!(effects.attach.as_deref(), Some("dplyr"));
    }

    #[test]
    fn if_false_takes_substitute_branch_and_folds_liveness() {
        // `character.only = FALSE` picks the `els` branch, which captures the
        // package symbol. The capture is inert, so its call slot folds to Quote.
        let call = first_call("library(dplyr, character.only = FALSE)");
        let effects = resolve(&attach_declaration(), &call, &CallContext::new()).unwrap();
        assert_eq!(effects.attach.as_deref(), Some("dplyr"));

        let arguments = effects.arguments.unwrap();
        assert_eq!(arguments.len(), 2);
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Quote { .. })
        ));
        assert!(arguments[1].is_none());
    }

    #[test]
    fn if_condition_falls_back_to_default_when_absent() {
        // With `character.only` absent, the condition reads its `FALSE` default,
        // so the `els` (substitute) branch names the captured symbol.
        let call = first_call("library(dplyr)");
        let effects = resolve(&attach_declaration(), &call, &CallContext::new()).unwrap();
        assert_eq!(effects.attach.as_deref(), Some("dplyr"));

        let arguments = effects.arguments.unwrap();
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Quote { .. })
        ));
    }

    #[test]
    fn if_condition_unresolved_drops_effect() {
        // A non-static `character.only` leaves the condition unresolved, so the
        // whole attach drops and nothing folds.
        let call = first_call("library(x, character.only = flag)");
        let effects = resolve(&attach_declaration(), &call, &CallContext::new()).unwrap();
        assert!(effects.attach.is_none());
        assert!(effects.arguments.is_none());
    }

    #[test]
    fn eval_operand_stays_live() {
        // The `then` branch reads the package via `Eval`, which leaves the slot
        // standard-eval: no fold, so `arguments` stays `None`.
        let call = first_call("library(x, character.only = TRUE)");
        let effects = resolve(&attach_declaration(), &call, &CallContext::new()).unwrap();
        // `x` isn't a static string, so no package resolves, but the point is
        // that a resolved-or-not `Eval` never folds its slot.
        assert!(effects.attach.is_none());
        assert!(effects.arguments.is_none());
    }

    /// A source declaration shaped like base `source`: path on formal 0, and an
    /// `envir` that maps `local` (formal 1, defaulting to `FALSE`) onto a target
    /// scope (`TRUE` -> caller's frame, `FALSE` -> global env).
    fn source_declaration() -> Declaration {
        Declaration::new(&["file", "local"])
            .formal_default(1, StaticValue::Bool(false))
            .source(DeclExpr::eval(0), DeclExpr::If {
                cond: RExpr::Eval(ArgumentRef(1)),
                then: Box::new(DeclExpr::Hole(RExpr::Env(EnvOp::ParentFrame))),
                els: Box::new(DeclExpr::Hole(RExpr::Env(EnvOp::GlobalEnv))),
            })
    }

    /// The single [`SourcedPath`] a source declaration resolves to.
    fn only_sourced(effects: &Effects) -> &SourcedPath {
        let sources = effects.source.as_ref().unwrap();
        assert_eq!(sources.len(), 1);
        &sources[0]
    }

    #[test]
    fn source_local_true_targets_current_scope() {
        let call = first_call("source(\"helpers.R\", local = TRUE)");
        let effects = resolve(&source_declaration(), &call, &CallContext::new()).unwrap();
        let sourced = only_sourced(&effects);
        assert_eq!(sourced.path, "helpers.R");
        assert_eq!(sourced.scope, NseScope::Current);
        // The path is read via `Eval`, so nothing folds.
        assert!(effects.arguments.is_none());
    }

    #[test]
    fn source_local_false_targets_global_scope() {
        let call = first_call("source(\"helpers.R\", local = FALSE)");
        let effects = resolve(&source_declaration(), &call, &CallContext::new()).unwrap();
        let sourced = only_sourced(&effects);
        assert_eq!(sourced.path, "helpers.R");
        assert_eq!(sourced.scope, NseScope::Global);
    }

    #[test]
    fn source_local_absent_defaults_to_global_scope() {
        // With `local` absent, the condition reads its `FALSE` default, so the
        // `els` branch (`globalenv()`) picks `Global`.
        let call = first_call("source(\"helpers.R\")");
        let effects = resolve(&source_declaration(), &call, &CallContext::new()).unwrap();
        let sourced = only_sourced(&effects);
        assert_eq!(sourced.path, "helpers.R");
        assert_eq!(sourced.scope, NseScope::Global);
    }

    #[test]
    fn source_named_path_resolves() {
        let call = first_call("source(file = \"helpers.R\")");
        let effects = resolve(&source_declaration(), &call, &CallContext::new()).unwrap();
        assert_eq!(only_sourced(&effects).path, "helpers.R");
    }

    #[test]
    fn source_non_static_local_drops_effect() {
        // A non-static `local =` leaves the `envir` condition unresolved, so the
        // source drops. What it rejects is a target environment we can't map to a
        // scope.
        let call = first_call("source(\"helpers.R\", local = some_env())");
        let effects = resolve(&source_declaration(), &call, &CallContext::new()).unwrap();
        assert!(effects.source.is_none());
    }
}
