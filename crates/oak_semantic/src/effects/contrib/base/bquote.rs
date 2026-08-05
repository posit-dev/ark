use aether_syntax::AnyRExpression;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::RIdentifierExt;

use crate::effects::CallContext;
use crate::effects::EffectHandler;
use crate::effects::Formals;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;

/// Handler for `bquote()`. It quotes its `expr` argument like `quote()`, but a
/// `.(X)` inside escapes back to evaluation, so `X` is a live sub-expression.
/// Recognizing `.()` is specific to bquote, so it lives in this handler rather
/// than in the shared [`ArgumentEffect`] vocabulary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BquoteHandler;

impl EffectHandler for BquoteHandler {
    type Output = ResolvedArgumentEffects;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<ResolvedArgumentEffects> {
        let formals: Formals = &["expr", "where", "splice"];
        let bound = ctx.bind_arguments(call, formals);

        // `..()` only splices under `splice = TRUE`.
        let splice = bound
            .get("splice")
            .and_then(|value| ctx.resolve_static_bool(value))
            .unwrap_or(false);

        Some(
            bound
                .arguments()
                .map(|(formal, value)| {
                    if formal != Some("expr") {
                        return None;
                    }
                    let holes = value
                        .map(|expr| unquote_holes(expr, splice))
                        .unwrap_or_default();
                    Some(ResolvedArgumentEffect::Quote { holes })
                })
                .collect(),
        )
    }
}

/// The unquote holes inside a bquote-quoted expression: the escaped argument of
/// each `.(foo)` call, plus each `..(foo)` when `splice` is on.
fn unquote_holes(expr: &AnyRExpression, splice: bool) -> Vec<AnyRExpression> {
    let mut holes = Vec::new();
    let mut preorder = expr.syntax().preorder();
    while let Some(event) = preorder.next() {
        let WalkEvent::Enter(node) = event else {
            continue;
        };
        let Some(call) = RCall::cast(node) else {
            continue;
        };
        if let Some(hole) = unquote_hole(&call, splice) {
            holes.push(hole);
            preorder.skip_subtree();
        }
    }
    holes
}

/// The escaped expression of a `.(foo)` unquote call, or a `..(foo)` splice
/// unquote when `splice` is on, or `None` when `call` isn't one. bquote's
/// unquote operator is the function `.`, and its splice unquote is `..`.
fn unquote_hole(call: &RCall, splice: bool) -> Option<AnyRExpression> {
    let AnyRExpression::RIdentifier(func) = call.function().ok()? else {
        return None;
    };
    let is_unquote = match func.name_text().as_str() {
        "." => true,
        ".." => splice,
        _ => false,
    };
    if !is_unquote {
        return None;
    }
    call.arguments().ok()?.items().iter().next()?.ok()?.value()
}
