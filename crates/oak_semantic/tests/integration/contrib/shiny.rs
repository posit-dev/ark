use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
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
