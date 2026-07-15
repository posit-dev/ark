use aether_syntax::AnyRArgumentName;
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
    pub fn nse(mut self, arg: usize, scope: NseScope, timing: NseTiming) -> Self {
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
}

/// One formal in a [`Declaration`]'s signature.
#[derive(Debug, Clone)]
pub struct FormalDef {
    pub name: String,
    /// The stub's default expression, consulted when the argument is absent.
    pub default: Option<RExpr>,
}

/// An arg-centric effect: one argument and how it evaluates.
#[derive(Debug, Clone)]
pub struct ArgumentEffect {
    /// Index into [`Declaration::formals`].
    pub arg: ArgumentRef,
    pub mode: EvalMode,
}

/// How an argument's own sub-expressions are treated.
#[derive(Debug, Clone, Copy)]
pub enum EvalMode {
    /// Captured unevaluated. bquote-style unquote holes stay a custom handler.
    Quote,
    /// Quote plus eval in a controlled scope, fused.
    Nse { scope: NseScope, timing: NseTiming },
}

/// An effect-centric effect: what a call does to the surrounding environment.
#[derive(Debug, Clone)]
pub enum EnvironmentEffect {
    /// Read and evaluate another file, injecting its top-level names. `guard`
    /// must resolve to a static bool or the effect drops. `local` isn't an
    /// operand of `path`, so it needs its own slot: resolving `path` never
    /// consults it, and `source("x.R", local = e)` would otherwise wrongly
    /// inject into the current scope.
    Source {
        path: DeclExpr,
        guard: Option<RExpr>,
    },
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

/// The bounded R interpreted inside a `.()` hole.
#[derive(Debug, Clone, Copy)]
pub enum RExpr {
    /// `.(x)`: force the argument to its value. A live use.
    Eval(ArgumentRef),
    /// `.(substitute(x))`: capture its expression. Inert, implies `x` is quoted.
    Substitute(ArgumentRef),
}

/// An index into the enclosing [`Declaration::formals`]. Name and position both
/// live in [`FormalDef`], so this is just the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentRef(pub usize);

/// Interpret a declaration against a call, producing the owned [`Effects`] the
/// builder consumes.
///
/// The arg-centric axis (`arguments`) matches the call against the declaration's
/// `formals` with [`match_signature`], then maps each matched formal to its
/// declared [`EvalMode`]. The effect-centric axis (`env`) does not contribute to
/// the result here.
pub fn resolve(declaration: &Declaration, call: &RCall, _ctx: &CallContext) -> Option<Effects> {
    let arguments = resolve_arguments(declaration, call);

    Some(Effects {
        arguments,
        ..Effects::default()
    })
}

/// Match the call against the declaration's formals, yielding the resolved effect
/// per call argument in order. `None` when the declaration names no arguments.
fn resolve_arguments(declaration: &Declaration, call: &RCall) -> Option<ResolvedArgumentEffects> {
    if declaration.arguments.is_empty() {
        return None;
    }

    let matched = match_signature(call, &declaration.formals);
    Some(
        matched
            .into_iter()
            .map(|formal| formal.and_then(|idx| declared_mode(declaration, idx).map(resolve_mode)))
            .collect(),
    )
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

fn resolve_mode(mode: &EvalMode) -> ResolvedArgumentEffect {
    match mode {
        EvalMode::Nse { scope, timing } => ResolvedArgumentEffect::Nse {
            scope: *scope,
            timing: *timing,
        },
        EvalMode::Quote => ResolvedArgumentEffect::Quote { holes: Vec::new() },
    }
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
fn match_signature(call: &RCall, formals: &[FormalDef]) -> Vec<Option<usize>> {
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
fn argument_name(arg: &RArgument) -> Option<String> {
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

    #[test]
    fn resolves_positional_nse_argument() {
        // `code` sits at positional index 1, so the block resolves to its Nse
        // effect and the leading `desc` stays plain.
        let call = first_call("test_that(\"d\", { x })");
        let declaration =
            Declaration::new(&["desc", "code"]).nse(1, NseScope::Nested, NseTiming::Eager);
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
            Declaration::new(&["desc", "code"]).nse(1, NseScope::Nested, NseTiming::Eager);
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
}
