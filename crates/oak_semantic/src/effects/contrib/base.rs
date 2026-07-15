use aether_syntax::AnyRExpression;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::RIdentifierExt;

use crate::effects::contrib::custom;
use crate::effects::contrib::Entry;
use crate::effects::declaration::argument_values;
use crate::effects::declaration::match_signature;
use crate::effects::AssignAnnotation;
use crate::effects::CallContext;
use crate::effects::EffectSite;
use crate::effects::Effects;
use crate::effects::FormalDef;
use crate::effects::Handler;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;

/// base R's custom contributions: the shapes a [`Declaration`] can't express.
/// `bquote` quotes with `.()` escape holes, and the assign family selects its
/// target environment in ways the grammar doesn't model yet. base R's plain NSE,
/// attach, and source effects live in `base.ty.R`.
///
/// [`Declaration`]: crate::effects::Declaration
pub(crate) fn entries() -> Vec<Entry> {
    vec![
        // `bquote` quotes `expr` like `quote()`, but its `.()` holes escape to
        // evaluation, so it needs a handler rather than a per-argument
        // declaration.
        custom("base", "bquote", &BquoteHandler),
        // `assign` and `delayedAssign` bind the name their `x` argument names.
        // The formals let `match_signature` recognize a named `x =` or `value =`
        // regardless of call position.
        custom("base", "assign", &AssignAnnotation {
            formals: &["x", "value", "pos", "envir", "inherits", "immediate"],
        }),
        custom("base", "delayedAssign", &AssignAnnotation {
            formals: &["x", "value", "eval.env", "assign.env"],
        }),
    ]
}

/// Handler for `bquote()`. It quotes its `expr` argument like `quote()`, but a
/// `.(X)` inside escapes back to evaluation, so `X` is a live sub-expression.
/// Recognizing `.()` is specific to bquote, so it lives in this handler rather
/// than in the declarative [`EvalMode`] vocabulary.
///
/// [`EvalMode`]: crate::effects::EvalMode
#[derive(Debug, Clone, Copy)]
pub(crate) struct BquoteHandler;

impl Handler for BquoteHandler {
    fn resolve(&self, site: EffectSite, ctx: &CallContext) -> Option<Effects> {
        let EffectSite::Call(call) = site else {
            return None;
        };

        // `bquote(expr, where, splice)`: only `expr` (formal 0) is quoted, and
        // `..()` splices only under `splice = TRUE` (formal 2). Matching the full
        // signature lets a named argument free the positional slots, so
        // `bquote(splice = TRUE, ..(x))` still binds `..(x)` to `expr`.
        let formals = bquote_formals();
        let matched = match_signature(call, &formals);
        let values = argument_values(call);

        let splice = matched
            .iter()
            .position(|formal| *formal == Some(2))
            .and_then(|i| values.get(i))
            .and_then(|value| value.as_ref())
            .and_then(|value| ctx.resolve_static_bool(value))
            .unwrap_or(false);

        let arguments: ResolvedArgumentEffects = matched
            .iter()
            .enumerate()
            .map(|(i, formal)| {
                if *formal != Some(0) {
                    return None;
                }
                let holes = values
                    .get(i)
                    .and_then(|value| value.as_ref())
                    .map(|expr| unquote_holes(expr, splice))
                    .unwrap_or_default();
                Some(ResolvedArgumentEffect::Quote { holes })
            })
            .collect();

        Some(Effects {
            arguments: Some(arguments),
            ..Effects::default()
        })
    }
}

fn bquote_formals() -> Vec<FormalDef> {
    ["expr", "where", "splice"]
        .into_iter()
        .map(|name| FormalDef {
            name: name.to_string(),
            default: None,
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use aether_parser::parse;
    use aether_parser::RParserOptions;
    use biome_rowan::WalkEvent;

    use super::*;

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
    fn splice_before_positional_expr_recognizes_hole() {
        // `splice = TRUE` is named, so the positional `..(x)` fills `expr` under
        // fill-remaining matching. Position-only matching missed it because the
        // named `splice` shifted the positional count off `expr`'s slot.
        let call = first_call("bquote(splice = TRUE, ..(x))");
        let effects = BquoteHandler
            .resolve(EffectSite::Call(&call), &CallContext::new())
            .unwrap();

        let arguments = effects.arguments.unwrap();
        let holes = arguments
            .iter()
            .find_map(|argument| match argument {
                Some(ResolvedArgumentEffect::Quote { holes }) => Some(holes),
                _ => None,
            })
            .unwrap();
        assert_eq!(holes.len(), 1);
    }
}
