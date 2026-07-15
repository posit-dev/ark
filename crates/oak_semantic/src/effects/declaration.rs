use aether_syntax::RCall;

use crate::effects::CallContext;
use crate::effects::Effects;
use crate::effects::Formal;
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
/// The arg-centric axis (`arguments`) maps each [`EvalMode`] to a
/// [`ResolvedArgumentEffect`], reusing [`CallContext::match_arguments`] to align
/// declared formals with the call. The effect-centric axis (`env`) does not
/// contribute to the result here.
pub fn resolve(declaration: &Declaration, call: &RCall, ctx: &CallContext) -> Option<Effects> {
    let arguments = resolve_arguments(declaration, call, ctx);

    Some(Effects {
        arguments,
        ..Effects::default()
    })
}

/// Match each declared argument against the call, yielding the resolved effect
/// per call argument in order. `None` when the declaration names no arguments.
fn resolve_arguments(
    declaration: &Declaration,
    call: &RCall,
    ctx: &CallContext,
) -> Option<ResolvedArgumentEffects> {
    if declaration.arguments.is_empty() {
        return None;
    }

    // One `Formal` per declared argument, keyed by its formal name and position.
    // `match_arguments` then maps each call argument to an index into this list,
    // which is parallel to `declaration.arguments`.
    let formals: Vec<Formal> = declaration
        .arguments
        .iter()
        .map(|effect| Formal {
            name: declaration.formals[effect.arg.0].name.as_str(),
            position: effect.arg.0,
        })
        .collect();

    let matched = ctx.match_arguments(call, &formals);
    Some(
        matched
            .into_iter()
            .map(|formal| formal.map(|i| resolve_mode(&declaration.arguments[i].mode)))
            .collect(),
    )
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
}
