use oak_semantic::semantic_index::AmbiguityReason;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticDiagnostic;
use oak_semantic::semantic_index::SymbolFlags;

use crate::common::index_with_base as index;

#[test]
fn test_nse_attach_enables_lazy_scope() {
    // `library(shiny)` attaches shiny in eager flow, so the later `reactive`
    // resolves to shiny's NSE annotation and pushes a lazy nested scope.
    let index = index(
        "\
library(shiny)
reactive({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);
    let reactive_scope = ScopeId::from(1);

    assert_eq!(index.attached_packages(), vec!["shiny"]);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
    assert_eq!(index.scope(reactive_scope).parent(), Some(file));
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(reactive_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_absent_leaves_callee_flat() {
    // Without the attach, shiny is unattached, so `reactive` doesn't resolve to
    // an NSE annotation and `x` stays at file scope.
    let index = index(
        "\
reactive({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    assert!(index.attached_packages().is_empty());
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_after_eager_callee_is_too_late() {
    // Flow order: at the eager `reactive` position shiny isn't attached yet, so
    // it is not NSE even though `library(shiny)` runs afterwards.
    let index = index(
        "\
reactive({
    x <- 1
})
library(shiny)
",
    );
    let file = ScopeId::from(0);

    assert_eq!(index.attached_packages(), vec!["shiny"]);
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_after_lazy_callee_is_visible() {
    // A callee inside a function runs at an unknown later time, so it sees the
    // end-of-file attach set. `reactive` inside `f` is resolved during the walk,
    // after the file scan attached shiny, so it is NSE even though `library`
    // comes after `f` textually.
    let index = index(
        "\
f <- function() {
    reactive({
        x <- 1
    })
}
library(shiny)
",
    );
    let f_scope = ScopeId::from(1);
    let reactive_scope = ScopeId::from(2);

    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
    assert_eq!(index.scope(reactive_scope).parent(), Some(f_scope));
}

#[test]
fn test_nse_attach_inside_eager_body_counts() {
    // The attach happens inside an eager `local` body, which the file scan
    // descends into, so shiny is attached in flow before the later top-level
    // `reactive`. This is where flow tracking beats a file-scope offset filter.
    let index = index(
        "\
local({
    library(shiny)
})
reactive({
    x <- 1
})
",
    );
    let local_scope = ScopeId::from(1);
    let reactive_scope = ScopeId::from(2);

    assert_eq!(index.attached_packages(), vec!["shiny"]);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
}

#[test]
fn test_nse_attach_does_not_leak_across_branches() {
    // `library(shiny)` in the `if` branch must not be visible in the `else`
    // branch. On the `else` path shiny never attached, so `reactive` is not NSE
    // and `x` stays at file scope. Without branch-scoped attaches the `else`
    // would wrongly see shiny leaked from the `if`.
    let index = index(
        "\
if (cond) {
    library(shiny)
} else {
    reactive({
        x <- 1
    })
}
",
    );
    let file = ScopeId::from(0);

    // The public attach list reports shiny (it's emitted per attach call, so it
    // sees the attach on the `if` path), but the `else` scan never saw shiny.
    assert_eq!(index.attached_packages(), vec!["shiny"]);
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_conditional_attach_drops_at_join() {
    // A `library(shiny)` on only the `if` path isn't attached on every path, so
    // it drops at the join, the same as a one-branch binding dropping from
    // `bound_so_far`. The later `reactive` then resolves against an attach set
    // without shiny, so it is not NSE and `x` stays at file scope.
    let index = index(
        "\
if (cond) library(shiny)
reactive({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    // The public attach list still reports shiny (emitted per attach call,
    // independent of the flow-join drop).
    assert_eq!(index.attached_packages(), vec!["shiny"]);
}

#[test]
fn test_nse_attach_on_both_branches_survives_join() {
    // Attached on both paths, so shiny is on every path and survives the join.
    // The later `reactive` resolves to shiny's NSE annotation and `x` is scoped.
    // Note that a more realistic version of this would be
    // `library(myshinyfork)` in the `else` branch. This would be sound but we
    // don't support this (unseen in the wild?) pattern.
    let index = index(
        "\
if (cond) library(shiny) else library(shiny)
reactive({
    x <- 1
})
",
    );
    let reactive_scope = ScopeId::from(1);

    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
    assert_eq!(
        index.symbols(reactive_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_in_loop_body_drops_after_loop() {
    // A `library(shiny)` in a `for` body isn't attached on every path (the body
    // may not run), so it drops after the loop. The later `reactive` is not NSE
    // and `x` stays at file scope.
    let index = index(
        "\
for (i in pkgs) library(shiny)
reactive({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_coguarded_attach_reaches_lazy_body() {
    // `f` is defined in the same branch that attached shiny, so reaching its
    // definition implies the guard held: if `f` exists at all, shiny is attached
    // whenever it runs. The join drops shiny from the linear set before the walk
    // scans `f`'s body, so the body inherits the attach from its definition
    // point instead.
    let index = index(
        "\
if (cond) {
    library(shiny)
    f <- function() reactive({
        x <- 1
    })
}
",
    );
    let f_scope = ScopeId::from(1);
    let reactive_scope = ScopeId::from(2);

    assert_eq!(index.scope_ids().count(), 3);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
    assert_eq!(
        index.symbols(reactive_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_coguarded_attach_reaches_doubly_nested_lazy_body() {
    // The inherited attach passes through each definition point, so it survives
    // however many function levels sit between the `library()` and the callee.
    let index = index(
        "\
if (cond) {
    library(shiny)
    f <- function() function() reactive({
        x <- 1
    })
}
",
    );
    let reactive_scope = ScopeId::from(3);

    assert_eq!(index.scope_ids().count(), 4);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
}

#[test]
fn test_nse_conditional_attach_not_inherited_by_unguarded_body() {
    // `f` is defined outside the guard, so its existence says nothing about
    // whether shiny attached. Nothing is inherited and the every-path set has
    // dropped shiny, so `reactive` is not NSE and `x` stays in `f`.
    let index = index(
        "\
if (cond) library(shiny)
f <- function() reactive({
    x <- 1
})
",
    );
    let f_scope = ScopeId::from(1);

    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.symbols(f_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_coguarded_attach_not_inherited_after_branch() {
    // The inherited attach belongs to bodies defined inside the branch, not to
    // later ones. `f` comes after the join, so it doesn't pick up shiny from the
    // branch that `g` was defined in.
    let index = index(
        "\
if (cond) {
    library(shiny)
    g <- function() 1
}
f <- function() reactive({
    x <- 1
})
",
    );
    let f_scope = ScopeId::from(2);

    assert_eq!(index.scope_ids().count(), 3);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.symbols(f_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_local_shadow_still_wins() {
    // A local `reactive` def shadows shiny's, so the call is not NSE even with
    // shiny attached.
    let index = index(
        "\
reactive <- function(x) x
library(shiny)
reactive({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);
    let fn_scope = ScopeId::from(1);

    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(index.scope(fn_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

// --- Conditional attach ambiguity diagnostics ---

#[test]
fn test_nse_conditional_attach_ambiguity_flagged() {
    // `library(shiny)` on only the `if` path drops at the join (see
    // `test_nse_conditional_attach_drops_at_join`), so `reactive` resolves as
    // effectless. That's ambiguous: on the `cond` path shiny was attached, so
    // the effect could have held. Flag it, pointing at the dropped attach.
    let source = "\
if (cond) library(shiny)
reactive({
    x <- 1
})
";
    let index = index(source);

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::EffectAmbiguity {
            name,
            call_range,
            reason:
                AmbiguityReason::ConditionalAttach {
                    package,
                    attach_range,
                },
        } => {
            assert_eq!(name, "reactive");
            assert_eq!(package, "shiny");

            let call_start = u32::from(call_range.start()) as usize;
            let call_end = u32::from(call_range.end()) as usize;
            assert_eq!(&source[call_start..call_end], "reactive({\n    x <- 1\n})");

            let attach_start = u32::from(attach_range.start()) as usize;
            let attach_end = u32::from(attach_range.end()) as usize;
            assert_eq!(&source[attach_start..attach_end], "library(shiny)");
        },
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

#[test]
fn test_nse_conditional_attach_names_the_responsible_package() {
    // Two attaches drop, but only shiny annotates `reactive`. The diagnostic
    // has to name shiny and point at its `library()`, not at whichever dropped
    // attach happens to come last.
    let source = "\
if (a) library(shiny)
if (b) library(testthat)
reactive({
    x <- 1
})
";
    let index = index(source);

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::EffectAmbiguity {
            reason:
                AmbiguityReason::ConditionalAttach {
                    package,
                    attach_range,
                },
            ..
        } => {
            assert_eq!(package, "shiny");
            let start = u32::from(attach_range.start()) as usize;
            let end = u32::from(attach_range.end()) as usize;
            assert_eq!(&source[start..end], "library(shiny)");
        },
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

#[test]
fn test_nse_coguarded_attach_no_ambiguity() {
    // Attach and use are co-guarded (see `test_nse_coguarded_attach_reaches_lazy_body`):
    // reaching `f`'s definition already implies shiny is attached, so
    // `attached_inherited` resolves `reactive` on the first probe. The
    // conditional-attach probe never runs, so nothing is flagged even though
    // the attach is conditional in isolation.
    let index = index(
        "\
if (cond) {
    library(shiny)
    f <- function() reactive({
        x <- 1
    })
}
",
    );

    assert!(index.diagnostics().is_empty());

    let f_scope = ScopeId::from(1);
    let reactive_scope = ScopeId::from(2);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
    assert_eq!(
        index.symbols(reactive_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_unconditional_attach_no_ambiguity() {
    // shiny is attached unconditionally, so nothing ever drops at a join and
    // the first resolution already succeeds. No diagnostic.
    let index = index(
        "\
library(shiny)
reactive({
    x <- 1
})
",
    );

    assert!(index.diagnostics().is_empty());
}

#[test]
fn test_nse_loop_attach_ambiguity_flagged() {
    // A `library(shiny)` in a `for` body drops after the loop (the body may not
    // run), the same as the branch case, so it's just as flaggable.
    let index = index(
        "\
for (i in pkgs) library(shiny)
reactive({
    x <- 1
})
",
    );

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::EffectAmbiguity {
            name,
            reason: AmbiguityReason::ConditionalAttach { package, .. },
            ..
        } => {
            assert_eq!(name, "reactive");
            assert_eq!(package, "shiny");
        },
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

#[test]
fn test_nse_attach_absent_no_ambiguity() {
    // shiny is never attached anywhere, so `attached_anywhere` is empty and the
    // conditional-attach probe finds nothing either. `reactive` stays
    // non-NSE, silently.
    let index = index(
        "\
reactive({
    x <- 1
})
",
    );

    assert!(index.diagnostics().is_empty());
}
