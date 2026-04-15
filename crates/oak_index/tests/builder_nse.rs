use aether_parser::parse;
use aether_parser::RParserOptions;
use oak_index::semantic_index;
use oak_index::semantic_index::DefinitionId;
use oak_index::semantic_index::NseScope;
use oak_index::semantic_index::ScopeId;
use oak_index::semantic_index::ScopeKind;
use oak_index::semantic_index::ScopeLaziness;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::SymbolFlags;
use oak_index::semantic_index::UseId;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    semantic_index(&parsed.tree())
}

// --- NSE scopes ---

#[test]
fn test_nse_local_creates_nested_eager_scope() {
    let index = index(
        "\
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // `local` is used at file scope
    assert_eq!(index.symbols(file).len(), 1);
    assert_eq!(
        index.symbols(file).get("local").unwrap().flags(),
        SymbolFlags::IS_USED
    );

    // `x` is defined inside the NSE scope, not at file level
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert_eq!(index.symbols(local_scope).len(), 1);
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_local_definition_not_in_parent() {
    // Definitions inside `local()` should NOT leak to the file scope.
    let index = index(
        "\
local({
    x <- 1
})
x
",
    );
    let file = ScopeId::from(0);

    // `x` at file scope is only IS_USED (from the bare `x` on the last line),
    // not IS_BOUND (from the assignment inside local).
    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_nse_evalq_no_scope_push() {
    // `evalq` is Current + Eager: no scope push, walk body in place.
    let index = index(
        "\
evalq({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    // Only the file scope exists (plus no child scopes)
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    // evalq is used, x is bound
    assert_eq!(index.symbols(file).len(), 2);
}

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
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(
        index.symbols(test_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_shadowed_name_no_scope() {
    // If `local` is locally defined (shadowed), it's not recognized as NSE.
    let index = index(
        "\
local <- identity
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    // `local` is defined at file scope, shadowing the base function.
    // No NSE scope should be created. `x` is defined at file scope.
    assert_eq!(
        index.symbols(file).get("local").unwrap().flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED)
    );
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_rewalk_moves_definitions() {
    // The re-walk should correctly move definitions from the parent scope
    // into the NSE child scope.
    let index = index(
        "\
x <- 0
local({
    y <- 1
})
z <- 2
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // File scope: x, z are bound; local is used; y is NOT in file scope
    assert!(index.symbols(file).get("x").is_some());
    assert!(index.symbols(file).get("z").is_some());
    assert!(index.symbols(file).get("local").is_some());
    assert!(index.symbols(file).get("y").is_none());

    // local scope: y is bound
    assert_eq!(
        index.symbols(local_scope).get("y").unwrap().flags(),
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
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(
        index.symbols(test_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_nested_function_inside_local() {
    // A function defined inside `local()` creates a nested Function scope.
    let index = index(
        "\
local({
    f <- function(x) x
})
",
    );
    let local_scope = ScopeId::from(1);
    let fun_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(index.scope(fun_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(fun_scope).parent(), Some(local_scope));
}

#[test]
fn test_nse_prescan_skips_nested_bodies() {
    // The pre-scan for the file scope should NOT include definitions from
    // inside `local()` bodies on the re-walk. This means a function defined
    // AFTER the local() call should not see `x` from inside local via the
    // pre-scan.
    let index = index(
        "\
local({
    x <- 1
})
f <- function() x
",
    );
    let file = ScopeId::from(0);
    let fun_scope = ScopeId::from(2);

    // `x` should NOT be in the file scope
    assert!(index.symbols(file).get("x").is_none());

    // In `f`, `x` is free and unbound — no enclosing snapshot should find it
    // in the file scope.
    assert_eq!(index.enclosing_bindings(fun_scope, UseId::from(0)), None);
}

#[test]
fn test_nse_eager_snapshot_precise() {
    // Eager NSE scope at file level should see a point-in-time snapshot:
    // only definitions that precede the call site, not later ones.
    let index = index(
        "\
x <- 1
local({
    x
})
x <- 2
",
    );
    let local_scope = ScopeId::from(1);

    // `x` inside local is free. Its enclosing snapshot should be eager
    // (point-in-time). At the call site, only `x <- 1` (DefinitionId 0) is
    // live. `x <- 2` (DefinitionId 2) comes after and should NOT be included.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(local_scope, UseId::from(0))
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(!bindings.may_be_unbound());
}

#[test]
fn test_nse_lazy_snapshot_accumulates() {
    // Lazy NSE scope (e.g. inside a function) should accumulate definitions
    // via watchers, just like function scopes do.
    let index = index(
        "\
x <- 1
f <- function() {
    x
}
x <- 2
",
    );
    let fun_scope = ScopeId::from(1);

    // Function is lazy: snapshot includes both x <- 1 and x <- 2.
    let (_, bindings) = index.enclosing_bindings(fun_scope, UseId::from(0)).unwrap();
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
}

#[test]
fn test_nse_current_lazy_routes_defs_to_parent() {
    // `rlang::on_load` is Current + Lazy: a scope is pushed (for lazy
    // snapshot resolution) but definitions route to the parent.
    let index = index(
        "\
rlang::on_load({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);
    let nse_scope = ScopeId::from(1);

    // `x` is routed to the file scope
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // The NSE scope exists with `Current + Lazy` kind
    assert_eq!(
        index.scope(nse_scope).kind(),
        ScopeKind::Nse(NseScope::Current, ScopeLaziness::Lazy)
    );
    assert_eq!(index.scope(nse_scope).parent(), Some(file));

    // `x` is not in the child scope's symbol table (routed to parent)
    assert!(index.symbols(nse_scope).get("x").is_none());
}

#[test]
fn test_nse_current_lazy_deferred_definition() {
    // `on_load` definitions are deferred (like `<<-`): they add to the set
    // of live definitions without shadowing what's already there.
    let index = index(
        "\
x <- 1
rlang::on_load({
    x <- 2
})
f <- function() x
",
    );
    let fun_scope = ScopeId::from(2);

    // `f` is lazy, so its snapshot for `x` should include BOTH defs:
    // `x <- 1` (file-level) and `x <- 2` (from on_load, deferred).
    // If on_load's definition shadowed, we'd only see `x <- 2`.
    let (enclosing_scope, bindings) = index.enclosing_bindings(fun_scope, UseId::from(0)).unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
}

#[test]
fn test_nse_rewalk_convergence_unmasked_call() {
    // Pathological case: redefining `local` inside a `local()` body unmasks
    // a later `local()` call on the re-walk. The convergence loop handles
    // this: the first re-walk discovers the second call, the second re-walk
    // has the correct pre-scan skip set.
    let index = index(
        "\
local({
    local <- identity
})
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);
    let first_local = ScopeId::from(1);
    let second_local = ScopeId::from(2);

    // Both calls create Nested + Eager scopes
    assert_eq!(
        index.scope(first_local).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(
        index.scope(second_local).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );

    // `local <- identity` is in the first scope, not at file level
    assert!(index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_USED));
    assert!(!index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));

    // `x <- 1` is in the second scope, not at file level
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(second_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_local_inside_function() {
    // `local()` inside a function: the function boundary is lazy, so the
    // eager snapshot precision of `local` is bounded by the function's
    // laziness. Free variables in `local` resolve through the function's
    // lazy snapshot.
    let index = index(
        "\
x <- 1
f <- function() {
    local({
        x
    })
}
x <- 2
",
    );
    let fun_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert_eq!(index.scope(fun_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(fun_scope));

    // `x` is free in the local scope, resolves through to the function scope,
    // then to the file scope. The function scope is lazy, so both defs are
    // visible despite `local` being eager.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(local_scope, UseId::from(0))
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
}

#[test]
fn test_nse_nested_local_scopes() {
    // Nested `local()` inside `local()`: both create child scopes.
    let index = index(
        "\
local({
    x <- 1
    local({
        y <- 2
    })
})
",
    );
    let file = ScopeId::from(0);
    let outer_local = ScopeId::from(1);
    let inner_local = ScopeId::from(2);

    assert_eq!(
        index.scope(outer_local).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(index.scope(outer_local).parent(), Some(file));
    assert_eq!(
        index.scope(inner_local).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(index.scope(inner_local).parent(), Some(outer_local));

    // Definitions land in their respective scopes
    assert!(index.symbols(file).get("x").is_none());
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(
        index.symbols(outer_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    assert!(index.symbols(outer_local).get("y").is_none());
    assert_eq!(
        index.symbols(inner_local).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_super_assignment_inside_local() {
    // `<<-` inside `local()` should target the grandparent (file scope),
    // not the local scope itself.
    let index = index(
        "\
x <- 1
local({
    x <<- 2
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // `x` is bound at file scope (from `x <- 1` and the `<<-`)
    assert!(index
        .symbols(file)
        .get("x")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));

    // `x` in the local scope is IS_SUPER_BOUND (the `<<-` site)
    assert!(index
        .symbols(local_scope)
        .get("x")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_SUPER_BOUND));
}

#[test]
fn test_nse_bare_test_that() {
    // Bare `test_that(...)` without `testthat::` prefix should be recognized
    // via `lookup_by_name` across all registered packages.
    let index = index(
        r#"
test_that("description", {
    x <- 1
})
"#,
    );
    let file = ScopeId::from(0);
    let test_scope = ScopeId::from(1);

    assert_eq!(
        index.scope(test_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, ScopeLaziness::Eager)
    );
    assert_eq!(index.scope(test_scope).parent(), Some(file));
    assert_eq!(
        index.symbols(test_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    assert!(index.symbols(file).get("x").is_none());
}
