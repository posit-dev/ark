use aether_syntax::AnyRExpression;
use aether_syntax::RArgumentNameClause;
use aether_syntax::RCall;
use aether_syntax::RIdentifier;
use aether_syntax::RParameter;
use biome_rowan::AstNode;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::RCallExt;
use oak_core::syntax_ext::RIdentifierExt;

use crate::effects::CallContext;
use crate::effects::EffectHandler;
use crate::effects::Formal;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;

/// Handler for `substitute()`. It quotes `expr` like `quote()`, but replaces
/// each symbol bound in its environment (the current frame by default) with what
/// that binding holds. Those substituted symbols are live uses of the frame
/// binding. The remaining part of the expression stays quoted and resolves
/// wherever the result is later evaluated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SubstituteHandler;

impl EffectHandler for SubstituteHandler {
    type Output = ResolvedArgumentEffects;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<ResolvedArgumentEffects> {
        // `substitute(expr, env)`: only `expr` (formal 0) is quoted, everything
        // else is a plain value.
        let formals = [
            Formal {
                name: "expr",
                position: 0,
            },
            Formal {
                name: "env",
                position: 1,
            },
        ];
        let matched = ctx.match_arguments(call, &formals);
        let expr_pos = matched.iter().position(|formal| *formal == Some(0))?;

        // Only the default `env`, the current frame, is one we can query. Any
        // explicit `env` names a frame we can't see into, so we bail to a plain
        // quote.
        //
        // TODO(nse, env): resolve an explicit env to its binding set (a
        // `list(...)`, `new.env()`, `parent.frame()`, an env-typed variable) once
        // environment captures and argument resolution land, and substitute
        // against that instead of bailing.
        let default_env = !matched.contains(&Some(1));

        // Substitution is disabled in the global environment
        let substitutes = default_env && !ctx.current_scope_is_global();

        let holes = if substitutes {
            call.argument_value(expr_pos)
                .map(|expr| substituted_symbols(&expr, ctx))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        // Effects align 1:1 with the call's arguments. Only the `expr` slot is
        // quoted, the rest is plain.
        let mut effects = vec![None; matched.len()];
        effects[expr_pos] = Some(ResolvedArgumentEffect::Quote { holes });
        Some(effects)
    }
}

/// The symbols in a `substitute`d expression that name a binding in the current
/// frame. R walks the whole parse tree and replaces every symbol the frame
/// binds, `$`/`@` members and both sides of `::` included, but never the tags
/// that name an argument (`f(x = .)`) or a formal (`function(x) .`). So we
/// collect every frame-bound `RIdentifier` outside those two tag positions. Each
/// becomes a hole the builder records as a use of that binding; the rest stay
/// inert.
fn substituted_symbols(expr: &AnyRExpression, ctx: &CallContext<'_>) -> Vec<AnyRExpression> {
    let mut holes = Vec::new();
    for event in expr.syntax().preorder() {
        let WalkEvent::Enter(node) = event else {
            continue;
        };
        let Some(ident) = RIdentifier::cast(node) else {
            continue;
        };
        if is_protected_name(&ident) {
            continue;
        }
        if ctx.is_bound(&ident.name_text(), false) {
            holes.push(AnyRExpression::RIdentifier(ident));
        }
    }
    holes
}

/// Whether `ident` is a tag naming an argument (`f(x = .)`) or a formal
/// parameter (`function(x) .`), the two positions R's `substitute` leaves
/// untouched. Each is the sole identifier child of its clause node, so the
/// parent kind identifies it.
fn is_protected_name(ident: &RIdentifier) -> bool {
    ident.syntax().parent().is_some_and(|parent| {
        RArgumentNameClause::can_cast(parent.kind()) || RParameter::can_cast(parent.kind())
    })
}
