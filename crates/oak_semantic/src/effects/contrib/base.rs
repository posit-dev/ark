use aether_syntax::AnyRExpression;
use aether_syntax::RArgumentNameClause;
use aether_syntax::RCall;
use aether_syntax::RIdentifier;
use aether_syntax::RParameter;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::RCallExt;
use oak_core::syntax_ext::RIdentifierExt;

use crate::effects::contrib::assign;
use crate::effects::contrib::nse;
use crate::effects::contrib::quoted;
use crate::effects::contrib::source;
use crate::effects::contrib::Entry;
use crate::effects::CallContext;
use crate::effects::EffectHandler;
use crate::effects::EffectsHandlers;
use crate::effects::Formal;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;
use crate::semantic_index::EvalEnv::Current;
use crate::semantic_index::EvalEnv::Nested;
use crate::semantic_index::EvalTiming::Eager;

pub(crate) static ENTRIES: &[Entry] = &[
    // base NSE
    nse!("base", "evalq", ("expr", 0, Current, Eager)),
    nse!("base", "local", ("expr", 0, Nested, Eager)),
    nse!("base", "with", ("expr", 1, Nested, Eager)),
    nse!("base", "with.default", ("expr", 1, Nested, Eager)),
    nse!("base", "within", ("expr", 1, Nested, Eager)),
    nse!("base", "within.data.frame", ("expr", 1, Nested, Eager)),
    // base quote
    quoted!("base", "quote", ("expr", 0)),
    // `bquote` quotes `expr` too, but its `.()` holes escape to evaluation, so
    // it needs a handler rather than a static per-argument effect.
    Entry {
        package: "base",
        function: "bquote",
        effects: EffectsHandlers {
            arguments: Some(&BquoteHandler),
            attach: None,
            source: None,
            assign: None,
        },
    },
    // `substitute` quotes `expr` too, but replaces the symbols its environment
    // binds, so it needs a handler that queries the scope rather than a static
    // per-argument effect.
    Entry {
        package: "base",
        function: "substitute",
        effects: EffectsHandlers {
            arguments: Some(&SubstituteHandler),
            attach: None,
            source: None,
            assign: None,
        },
    },
    // base attach. `library`/`require` share `LibraryHandler` (below).
    attach_entry("library"),
    attach_entry("require"),
    // base source
    source!("base", "source", 0),
    // base assign
    assign!("base", "assign", 0),
    assign!("base", "delayedAssign", 0),
];

/// Handler for `bquote()`. It quotes its `expr` argument like `quote()`, but a
/// `.(X)` inside escapes back to evaluation, so `X` is a live sub-expression.
/// Recognizing `.()` is specific to bquote, so it lives in this handler rather
/// than in the shared [`ArgumentEffect`] vocabulary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BquoteHandler;

impl EffectHandler for BquoteHandler {
    type Output = ResolvedArgumentEffects;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<ResolvedArgumentEffects> {
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

        Some(
            matched
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

/// Build the attach [`Entry`] for a base function served by [`LibraryHandler`].
const fn attach_entry(function: &'static str) -> Entry {
    Entry {
        package: "base",
        function,
        effects: EffectsHandlers {
            arguments: None,
            attach: Some(&LibraryHandler),
            source: None,
            assign: None,
        },
    }
}

/// Handler for `library()` and `require()`. Names the attached package from the
/// first argument, read as quoted (the symbol or string as written, so
/// `library(dplyr)` attaches `dplyr`). `character.only = TRUE` flips that
/// argument to standard eval (a value to resolve, `library(pkg, character.only =
/// TRUE)`), matching R. That flag is specific to these callees, so it lives in
/// this handler rather than the shared attach vocabulary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LibraryHandler;

impl EffectHandler for LibraryHandler {
    type Output = String;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<String> {
        // `character.only` sits at signature position 4 in both callees; in
        // practice it's passed by name.
        let formals = [
            Formal {
                name: "package",
                position: 0,
            },
            Formal {
                name: "character.only",
                position: 4,
            },
        ];
        let matched = ctx.match_arguments(call, &formals);

        let args = call.arguments().ok()?;
        let values: Vec<Option<AnyRExpression>> = args
            .items()
            .iter()
            .map(|item| item.ok().and_then(|arg| arg.value()))
            .collect();

        let package = matched
            .iter()
            .position(|formal| *formal == Some(0))
            .and_then(|i| values.get(i))
            .and_then(|value| value.as_ref())?;

        let character_only = matched
            .iter()
            .position(|formal| *formal == Some(1))
            .and_then(|i| values.get(i))
            .and_then(|value| value.as_ref())
            .and_then(|value| ctx.resolve_static_bool(value))
            .unwrap_or(false);

        if character_only {
            ctx.resolve_static_string(package)
        } else {
            ctx.resolve_quoted_symbol_or_string(package)
        }
    }
}
