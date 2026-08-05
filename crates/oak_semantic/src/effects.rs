use std::sync::LazyLock;

use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
use aether_syntax::RArgument;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use biome_rowan::AstPtr;
use biome_rowan::AstSeparatedList;
// Re-exported so consumers building an `AssignBinding` (custom `AssignHandler`s)
// can name the `name_expr` field's type without depending on oak_core directly.
pub use oak_core::range::RangedAstPtr;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use rustc_hash::FxHashMap;

use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;

/// Per-package tables of which functions carry effects. Private data behind the
/// `lookup`/`annotates` query API below.
mod contrib;

/// Registry entries keyed by function name so they can be queried by `lookup()`
/// (package plus function) and `annotates()` (function only) probe on. Entries
/// for a name carried by several packages (e.g. `defer()` in both withr and
/// rlang) are kept in registry order, so `lookup` breaks a tie the same way a
/// scan of `REGISTRY` would.
static INDEX: LazyLock<FxHashMap<&'static str, Vec<(&'static str, &'static EffectsHandlers)>>> =
    LazyLock::new(|| {
        let mut index: FxHashMap<&'static str, Vec<(&'static str, &'static EffectsHandlers)>> =
            FxHashMap::default();
        for package in contrib::REGISTRY {
            for entry in package.functions {
                index
                    .entry(entry.function)
                    .or_default()
                    .push((package.name, &entry.effects));
            }
        }
        index
    });

/// Effects of a call, resolved against the call site.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    /// Per-argument evaluation effects, resolved against the call and aligned
    /// 1:1 with its arguments. `None` at a slot means a plain (standard-eval)
    /// argument.
    pub arguments: Option<ResolvedArgumentEffects>,
    /// Attach a package
    pub attach: Option<String>,
    /// Source one or more paths. A vector so a collation-style callee can name
    /// several; base `source` resolves to one.
    pub source: Option<Vec<SourcePath>>,
    /// Bind one or more names in the current scope (`assign("x", value)`). A
    /// vector so a multi-binding callee stays expressible; base `assign` and
    /// `delayedAssign` resolve to one.
    pub assign: Option<Vec<AssignBinding>>,
}

/// One name an assign call binds, with the syntax handles its consumers need.
/// - The bound name feeds the symbol table.
/// - `name_expr` anchors the goto target and carries a trimmed range that can
///   be matched against a cursor (e.g. for goto/rename).
/// - `value_expr` is what a type checker infers the binding's type from (`None`
///   with no value argument).
/// - `target` tells the walk whether the bound name is also read here.
#[derive(Debug, Clone)]
pub struct AssignBinding {
    pub name: String,
    pub name_expr: RangedAstPtr<AnyRExpression>,
    pub value_expr: Option<AstPtr<AnyRExpression>>,
    pub target: TargetAccess,
}

/// The handlers that compute a function's effects.
#[derive(Debug, Clone, Copy)]
pub struct EffectsHandlers {
    pub arguments: Option<&'static dyn EffectHandler<Output = ResolvedArgumentEffects>>,
    pub attach: Option<&'static dyn EffectHandler<Output = String>>,
    pub source: Option<&'static dyn EffectHandler<Output = Vec<SourcePath>>>,
    pub assign: Option<&'static dyn AssignHandler>,
}

/// Look up the effect handlers of a `(package, function)` pair.
pub fn lookup(package: &str, function: &str) -> Option<&'static EffectsHandlers> {
    INDEX
        .get(function)?
        .iter()
        .find(|(entry_package, _)| *entry_package == package)
        .map(|(_, effects)| *effects)
}

/// Whether any registry entry annotates `name`. This is the bare-callee front
/// gate: an unannotated name can't resolve to an effect no matter which provider
/// wins, so recognition skips resolution entirely.
pub fn annotates(name: &str) -> bool {
    INDEX.contains_key(name)
}

/// HACK: This matches a `sourceDir()` call syntactically. See `?source` for the
/// definition of `sourceDir()` that people copy around:
/// https://github.com/search?q=sourceDir+language%3AR&type=code
/// This is a stopgap workaround until we can infer source effects around a
/// `list.files()` loop.
///
/// The copied `sourceDir()` idiom leaves `list.files()` at its
/// `recursive = FALSE` default, so nested scripts are excluded.
pub fn source_dir_idiom(name: &str) -> Option<&'static EffectsHandlers> {
    if name != "sourceDir" {
        return None;
    }
    Some(&EffectsHandlers {
        arguments: None,
        attach: None,
        source: Some(&SourceAnnotation {
            formals: &["path"],
            path: "path",
            target: SourceTarget::Dir(DirWalk::Shallow),
            default_path: None,
        }),
        assign: None,
    })
}

/// Resolver for an effect of a call.
///
/// The single interface behind every effect kind (NSE, attach, source).
///
/// Handlers are contributed statically for now (a `&'static dyn` in the
/// registry), so the trait is `Sync`, which every registry `static` needs.
pub trait EffectHandler: std::fmt::Debug + Sync {
    type Output;

    /// Resolve this effect for `call`, or `None` when the call isn't in a shape
    /// this handler recognizes.
    ///
    /// `ctx` provides semantic resolution, e.g. resolve an argument to a
    /// statically known string or boolean.
    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<Self::Output>;
}

/// Where an effect is invoked. Most effects are only ever calls but an Assign
/// effect can also be a binding operator (`x %<>% f`). [`AssignHandler`] takes
/// this to disambiguate rather than a bare call.
pub enum EffectSite<'a> {
    Call(&'a RCall),
    Operator(&'a RBinaryExpression),
}

/// Resolver for an assign-like effect.
///
/// Separate from [`EffectHandler`] because an assign has two invocation shapes,
/// a call (`assign("x", v)`) and a binding operator (`x %<>% f`).
///
/// Contributed statically like [`EffectHandler`], so it's `Sync` for the
/// registry `static`s.
pub trait AssignHandler: std::fmt::Debug + Sync {
    fn resolve(&self, site: EffectSite, ctx: &CallContext<'_>) -> Option<Vec<AssignBinding>>;
}

/// Scope state a handler needs that the call syntax alone can't answer, backed
/// by the builder's flow-precise binding tables.
///
/// `substitute` uses this to tell which symbols in its argument name a binding
/// in the current scope (so they resolve here, against substitute's env) from
/// those that stay quoted (so they resolve wherever the result is later
/// evaluated).
pub trait ScopeContext {
    /// Whether `name` is bound in the current scope. With `inherits`, also
    /// counts bindings inherited from enclosing scopes, mirroring R's
    /// `get(..., inherits=)`.
    fn is_bound(&self, name: &str, inherits: bool) -> bool;

    /// Whether the current scope is the global (file) scope. R's `substitute`
    /// substitutes nothing in the global environment, so a handler falls back to
    /// a plain quote there.
    fn is_global(&self) -> bool;
}

/// Whether an assign effect reads its target before writing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetAccess {
    /// Writes the target without reading it, as in `x <- value`.
    Write,
    /// Reads the target before rebinding it. `x %<>% f()` expands to
    /// `x <- x %>% f()`, so `x` is a use and a definition.
    ReadWrite,
}

/// Context for effect handlers.
///
/// Allows querying the properties or static values of arguments, and the
/// binding state of the surrounding scope.
#[derive(Default)]
pub struct CallContext<'a> {
    scope: Option<&'a dyn ScopeContext>,
}

impl<'a> CallContext<'a> {
    /// A context backed by the builder's scope state, for handlers that query
    /// bindings (`substitute`).
    pub fn with_bindings(bindings: &'a dyn ScopeContext) -> Self {
        Self {
            scope: Some(bindings),
        }
    }

    /// Whether `name` is bound in the current scope (see
    /// [`ScopeQuery::is_bound`]). Without a bindings backing (a [`Default`]
    /// context) we can't tell, so we answer "unbound", the choice that leaves a
    /// symbol quoted rather than treating it as a use.
    pub fn is_bound(&self, name: &str, inherits: bool) -> bool {
        self.scope
            .is_some_and(|scope| scope.is_bound(name, inherits))
    }

    /// Whether the current scope is the global (file) scope (see
    /// [`ScopeQuery::is_global_scope`]). Without a bindings backing (a
    /// [`Default`] context) we assume global, so `substitute` degrades to a
    /// plain quote (its no-substitution behaviour).
    pub fn current_scope_is_global(&self) -> bool {
        self.scope.is_none_or(|scope| scope.is_global())
    }

    /// Match `call` arguments to `formals`, returning a formal index for each
    /// call argument. Exact named matches consume slots first, then unnamed
    /// arguments fill unconsumed slots in signature order.
    fn match_arguments(&self, call: &RCall, formals: Formals) -> Vec<Option<usize>> {
        let Ok(args) = call.arguments() else {
            return Vec::new();
        };
        let items = args.items();

        let arg_count = items.iter().count();
        let mut matched: Vec<Option<usize>> = vec![None; arg_count];
        let mut consumed = vec![false; formals.len()];

        // Named pass
        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            if let Some(formal_idx) = match_named(&arg, formals, &consumed) {
                consumed[formal_idx] = true;
                matched[i] = Some(formal_idx);
            }
        }

        // Positional pass. Only unnamed args reach the match, and none of them
        // were set by the named pass, so no need to re-check `matched[i]`.
        let mut next_slot = 0usize;
        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            if arg.name_clause().is_some() {
                continue;
            }
            while next_slot < consumed.len() && consumed[next_slot] {
                next_slot += 1;
            }
            let Some(formal_idx) = (next_slot < formals.len()).then_some(next_slot) else {
                continue;
            };
            consumed[formal_idx] = true;
            matched[i] = Some(formal_idx);
            next_slot += 1;
        }

        matched
    }

    /// Bind `call`'s arguments to `formals`, for handlers that read arguments by
    /// formal name rather than by call position.
    pub fn bind_arguments(&self, call: &RCall, formals: Formals) -> BoundArguments {
        let matched = self.match_arguments(call, formals);
        let values: Vec<Option<AnyRExpression>> = match call.arguments() {
            Ok(args) => args
                .items()
                .iter()
                .map(|item| item.ok().and_then(|arg| arg.value()))
                .collect(),
            Err(_) => Vec::new(),
        };
        BoundArguments {
            formals,
            bound: matched.into_iter().zip(values).collect(),
        }
    }

    /// Statically evaluate an argument's value expression to a string. `None`
    /// when it's dynamic.
    pub fn resolve_static_string(&self, value: &AnyRExpression) -> Option<String> {
        match value {
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
            // Static resolution of expressions is not implemented yet
            _ => None,
        }
    }

    /// Read a quoted name argument. E.g. the LHS of an Assign operator.
    pub fn resolve_quoted_symbol_or_string(&self, value: &AnyRExpression) -> Option<String> {
        match value {
            AnyRExpression::RIdentifier(ident) => Some(ident.name_text()),
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
            _ => None,
        }
    }

    /// Statically evaluate an argument's value expression to a bool.
    pub fn resolve_static_bool(&self, value: &AnyRExpression) -> Option<bool> {
        match value {
            AnyRExpression::RTrueExpression(_) => Some(true),
            AnyRExpression::RFalseExpression(_) => Some(false),
            // Static resolution of expressions is not implemented yet
            _ => None,
        }
    }
}

/// The initial formal names needed to match the arguments this handler reads.
/// Include every earlier slot so unnamed arguments bind correctly. Stop before
/// `...`, because later R formals are matched by name rather than position.
pub type Formals = &'static [&'static str];

/// A call's arguments indexed by the formals they match.
pub struct BoundArguments {
    formals: Formals,
    /// One entry per call argument, in call order: the formal it matched and its
    /// value expression.
    bound: Vec<(Option<usize>, Option<AnyRExpression>)>,
}

impl BoundArguments {
    /// The expression bound to `formal`. Returns `None` when the call uses the
    /// default or the handler did not declare the formal.
    pub fn get(&self, formal: &str) -> Option<&AnyRExpression> {
        let formal_idx = self.formals.iter().position(|name| *name == formal)?;
        self.bound
            .iter()
            .find(|(matched_idx, _)| *matched_idx == Some(formal_idx))?
            .1
            .as_ref()
    }

    /// Returns each call argument's matched formal and expression in call order.
    pub fn arguments(&self) -> impl Iterator<Item = (Option<&str>, Option<&AnyRExpression>)> + '_ {
        self.bound
            .iter()
            .map(|(matched_idx, value)| (matched_idx.map(|idx| self.formals[idx]), value.as_ref()))
    }

    pub fn len(&self) -> usize {
        self.bound.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }
}

/// A call's resolved argument effects: for each argument in call order, the
/// effect it resolved to, or `None` for a plain (standard-eval) argument.
pub type ResolvedArgumentEffects = Vec<Option<ResolvedArgumentEffect>>;

/// The resolved, per-call effect of one argument. The builder consumes these.
#[derive(Debug, Clone)]
pub enum ResolvedArgumentEffect {
    /// Quote the argument, then evaluate it in `env`. `timing` says whether
    /// that happens eagerly at the call site (`evalq()`, `local()`) or later
    /// at an unknown time (`on_load()`, `reactive()`).
    EvalQ { env: EvalEnv, timing: EvalTiming },
    /// Captured unevaluated. `holes` are the sub-expressions that escape back to
    /// evaluation (e.g. bquote's `.()` contents), walked normally; everything
    /// else in the argument is inert. Empty for a plain `quote()`.
    Quote { holes: Vec<AnyRExpression> },
}

/// Declares how a function evaluates its annotated arguments, and serves as the
/// default [`EffectHandler`] for it by matching the declaration to a call.
#[derive(Debug, Clone, Copy)]
pub struct ArgumentsAnnotation {
    pub formals: Formals,
    pub arguments: &'static [Argument],
}

#[derive(Debug)]
pub struct Argument {
    pub name: &'static str,
    pub effect: ArgumentEffect,
}

/// What static operation an argument's evaluation calls for, mirroring R's
/// evaluation model.
#[derive(Debug, Clone, Copy)]
pub enum ArgumentEffect {
    /// Quote the argument, then evaluate it in `env`. `timing` says whether
    /// that happens eagerly at the call site (`evalq()`, `local()`) or later
    /// at an unknown time (`on_load()`, `reactive()`).
    EvalQ { env: EvalEnv, timing: EvalTiming },
    /// Captured unevaluated, so its symbols are not uses and nothing in it runs.
    /// `quote`. A function that unquotes (`bquote()`, whose `.()` holes escape)
    /// can't be expressed statically, and must use a custom handler instead of
    /// this variant.
    Quote,
}

impl ArgumentEffect {
    fn resolve(self) -> ResolvedArgumentEffect {
        match self {
            ArgumentEffect::EvalQ { env, timing } => ResolvedArgumentEffect::EvalQ { env, timing },
            ArgumentEffect::Quote => ResolvedArgumentEffect::Quote { holes: Vec::new() },
        }
    }
}

impl EffectHandler for ArgumentsAnnotation {
    type Output = ResolvedArgumentEffects;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<ResolvedArgumentEffects> {
        let bound = ctx.bind_arguments(call, self.formals);
        Some(
            bound
                .arguments()
                .map(|(formal, _)| {
                    let formal = formal?;
                    self.arguments
                        .iter()
                        .find(|argument| argument.name == formal)
                        .map(|argument| argument.effect.resolve())
                })
                .collect(),
        )
    }
}

/// A path a source call names, and what that path points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePath {
    pub path: String,
    pub target: SourceTarget,
}

/// What a source function's path argument points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTarget {
    /// A single file, as base `source()` takes.
    File,
    /// R files in a directory. [`DirWalk`] determines whether descendants count.
    Dir(DirWalk),
    /// A file or directory. `source()` takes only files, while
    /// `targets::tar_source()` takes both.
    FileOrDir(DirWalk),
}

/// Controls whether directory source targets include descendants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirWalk {
    /// Direct children only, matching `list.files()` without `recursive = TRUE`.
    Shallow,
    /// Every R file below the directory, matching `list.files(recursive = TRUE)`.
    Recursive,
}

/// Declares how a source function (`source()`) names what it reads, and serves
/// as the default [`EffectHandler`] for it by pulling that path out of a call.
#[derive(Debug, Clone, Copy)]
pub struct SourceAnnotation {
    pub formals: Formals,
    pub path: &'static str,
    /// Whether that argument names a file or a directory.
    pub target: SourceTarget,
    /// Default path if no argument is suppolied (`tar_source()` defaults to
    /// `files = "R"`).
    pub default_path: Option<&'static str>,
}

impl EffectHandler for SourceAnnotation {
    type Output = Vec<SourcePath>;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<Vec<SourcePath>> {
        let bound = ctx.bind_arguments(call, self.formals);

        if let Some(local) = bound.get("local") {
            match local {
                AnyRExpression::RTrueExpression(_) | AnyRExpression::RFalseExpression(_) => {},
                // Only literal `TRUE` and `FALSE` make the source scope statically
                // known.
                _ => return None,
            }
        }

        let path = match bound.get(self.path) {
            // An explicit dynamic path suppresses the default.
            Some(value) => ctx.resolve_static_string(value)?,
            None => self.default_path?.to_string(),
        };

        Some(vec![SourcePath {
            path,
            target: self.target,
        }])
    }
}

/// Declares how an assign function (`assign()`, `delayedAssign()`) names the
/// variable it binds, and serves as the default [`EffectHandler`] for it by
/// pulling that name out of a call.
#[derive(Debug, Clone, Copy)]
pub struct AssignAnnotation {
    pub formals: Formals,
    /// Formal holding the bound name.
    pub name: &'static str,
    /// Formal holding the bound value.
    pub value: &'static str,
    /// Formals that select where the binding lands. `delayedAssign()` takes two
    /// environments and only `assign.env` is one of these.
    pub target_env: Formals,
}

impl AssignHandler for AssignAnnotation {
    fn resolve(&self, site: EffectSite, ctx: &CallContext<'_>) -> Option<Vec<AssignBinding>> {
        let EffectSite::Call(call) = site else {
            return None;
        };
        let bound = ctx.bind_arguments(call, self.formals);

        // An explicit target environment binds outside the current scope, which
        // we don't currently support.
        if self
            .target_env
            .iter()
            .any(|formal| bound.get(formal).is_some())
        {
            return None;
        }

        let name_expr = bound.get(self.name)?;
        let name = ctx.resolve_static_string(name_expr)?;

        Some(vec![AssignBinding {
            name,
            name_expr: RangedAstPtr::new(name_expr),
            value_expr: bound.get(self.value).map(AstPtr::new),
            target: TargetAccess::Write,
        }])
    }
}

/// Handler for a binding operator (`x %<>% f()`, `x %<~% expr`, `x := expr`).
#[derive(Debug, Clone, Copy)]
pub struct BindingOperatorHandler {
    /// Whether the target is also read (compound operators like `%<>%`).
    pub target: TargetAccess,
}

impl AssignHandler for BindingOperatorHandler {
    fn resolve(&self, site: EffectSite, ctx: &CallContext<'_>) -> Option<Vec<AssignBinding>> {
        let EffectSite::Operator(bin) = site else {
            return None;
        };
        let left = bin.left().ok()?;
        let right = bin.right().ok()?;

        let name = ctx.resolve_quoted_symbol_or_string(&left)?;

        Some(vec![AssignBinding {
            name,
            name_expr: RangedAstPtr::new(&left),
            value_expr: Some(AstPtr::new(&right)),
            target: self.target,
        }])
    }
}

/// Match a named argument against `formals`. Returns the index of the matched
/// formal.
///
/// Should we do partial argument matching? Or rely on partial matching being linted?
fn match_named(arg: &RArgument, formals: Formals, consumed: &[bool]) -> Option<usize> {
    let clause = arg.name_clause()?;
    let name = clause.name().ok()?;
    let name_text = match &name {
        AnyRArgumentName::RIdentifier(ident) => ident.name_text(),
        AnyRArgumentName::RStringValue(s) => s.string_text()?,
        _ => return None,
    };
    formals
        .iter()
        .enumerate()
        .find(|(i, formal_name)| !consumed[*i] && **formal_name == name_text.as_str())
        .map(|(i, _)| i)
}
