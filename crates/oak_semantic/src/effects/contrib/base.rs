use aether_syntax::AnyRExpression;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::RIdentifierExt;

use crate::effects::contrib::custom;
use crate::effects::contrib::declared;
use crate::effects::contrib::Entry;
use crate::effects::ArgumentRef;
use crate::effects::AssignAnnotation;
use crate::effects::CallContext;
use crate::effects::DeclExpr;
use crate::effects::Declaration;
use crate::effects::EffectSite;
use crate::effects::Effects;
use crate::effects::Formal;
use crate::effects::Handler;
use crate::effects::RExpr;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;
use crate::effects::StaticValue;
use crate::semantic_index::NseScope::Current;
use crate::semantic_index::NseScope::Nested;
use crate::semantic_index::NseTiming::Eager;

pub(crate) fn entries() -> Vec<Entry> {
    vec![
        // base NSE
        declared(
            "base",
            "evalq",
            Declaration::new(&["expr"]).nse(0, Current, Eager),
        ),
        declared(
            "base",
            "local",
            Declaration::new(&["expr"]).nse(0, Nested, Eager),
        ),
        declared(
            "base",
            "with",
            Declaration::new(&["data", "expr"]).nse(1, Nested, Eager),
        ),
        declared(
            "base",
            "with.default",
            Declaration::new(&["data", "expr"]).nse(1, Nested, Eager),
        ),
        declared(
            "base",
            "within",
            Declaration::new(&["data", "expr"]).nse(1, Nested, Eager),
        ),
        declared(
            "base",
            "within.data.frame",
            Declaration::new(&["data", "expr"]).nse(1, Nested, Eager),
        ),
        // base quote
        declared("base", "quote", Declaration::new(&["expr"]).quote(0)),
        // `bquote` quotes `expr` too, but its `.()` holes escape to evaluation,
        // so it needs a handler rather than a per-argument declaration.
        custom("base", "bquote", &BquoteHandler),
        // base attach. `library` reads its `package` as quoted (the symbol or
        // string as written, so `library(dplyr)` attaches `dplyr`), unless
        // `character.only = TRUE` flips it to a string to evaluate. The `if`
        // encodes that flip, mirroring library's own `character.only` branch.
        declared(
            "base",
            "library",
            library_declaration(&["package", "help", "pos", "lib.loc", "character.only"]),
        ),
        // `require` shares library's attach shape but its own signature:
        // `character.only` happens to sit at index 4 in both.
        declared(
            "base",
            "require",
            library_declaration(&[
                "package",
                "lib.loc",
                "quietly",
                "warn.conflicts",
                "character.only",
            ]),
        ),
        // base source. The `local =` guard must be a static bool or the source
        // drops: `source("x.R", local = e)` targets some other environment, so
        // its names aren't ours to inject. `path` never consults `local`, hence
        // the separate guard slot.
        declared(
            "base",
            "source",
            Declaration::new(&["file", "local"])
                .formal_default(1, StaticValue::Bool(false))
                .source(DeclExpr::eval(0), Some(RExpr::Eval(ArgumentRef(1)))),
        ),
        // base assign
        custom("base", "assign", &AssignAnnotation { position: 0 }),
        custom("base", "delayedAssign", &AssignAnnotation { position: 0 }),
    ]
}

/// A `library`/`require` declaration over `formals`. Both attach the same way,
/// so they differ only in their signature. `character.only` sits at index 4 in
/// both, defaulting to `FALSE`.
fn library_declaration(formals: &[&str]) -> Declaration {
    Declaration::new(formals)
        .formal_default(4, StaticValue::Bool(false))
        .attach(DeclExpr::If {
            cond: RExpr::Eval(ArgumentRef(4)),
            then: Box::new(DeclExpr::eval(0)),
            els: Box::new(DeclExpr::substitute(0)),
        })
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

        // `bquote(expr, where, splice)`: only `expr` (the first positional) is
        // quoted. The other arguments are ordinary values.
        let formals = [
            Formal {
                name: "expr",
                position: 0,
            },
            Formal {
                name: "splice",
                position: 2,
            },
        ];
        let matched = ctx.match_arguments(call, &formals);

        let args = call.arguments().ok()?;
        let values: Vec<Option<AnyRExpression>> = args
            .items()
            .iter()
            .map(|item| item.ok().and_then(|arg| arg.value()))
            .collect();

        // `..()` only splices under `splice = TRUE`.
        let splice = matched
            .iter()
            .position(|formal| *formal == Some(1))
            .and_then(|i| values.get(i))
            .and_then(|value| value.as_ref())
            .and_then(|value| ctx.resolve_static_bool(value))
            .unwrap_or(false);

        let arguments: ResolvedArgumentEffects = matched
            .into_iter()
            .enumerate()
            .map(|(i, formal)| {
                // Only `expr` (formal 0) is quoted
                if formal != Some(0) {
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
