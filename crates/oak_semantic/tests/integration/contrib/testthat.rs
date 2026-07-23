use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SymbolFlags;

use crate::common::index_with_base as index;

#[test]
fn test_nse_namespace_qualified_call() {
    // `testthat::test_that` should be recognized via namespace resolution.
    let index = index(
        r#"
testthat::test_that("description", {
    x <- 1
})
"#,
    );
    let file = ScopeId::from(0);
    let test_scope = ScopeId::from(1);

    // File scope has no symbols (namespace expressions don't record uses)
    assert_eq!(index.symbols(file).len(), 0);

    // Test scope contains `x`
    assert_eq!(
        index.scope(test_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(
        index.symbols(test_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_test_that_second_arg() {
    // `test_that` has the scoped arg at position 1 (the `code` parameter).
    // The first argument (description) should be processed normally.
    let index = index(
        r#"
testthat::test_that("description", {
    x <- 1
    y
})
"#,
    );
    let test_scope = ScopeId::from(1);

    // Inside the test scope
    assert_eq!(
        index.symbols(test_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    assert_eq!(
        index.symbols(test_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_nse_named_argument_matching() {
    // Named argument matching: `code = {...}` should be recognized.
    let index = index(
        r#"
testthat::test_that(code = {
    x <- 1
}, desc = "foo")
"#,
    );
    let test_scope = ScopeId::from(1);

    assert_eq!(
        index.scope(test_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(
        index.symbols(test_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}
