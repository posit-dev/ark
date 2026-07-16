use aether_parser::parse;
use aether_parser::RParserOptions;
use oak_semantic::build_index;
use oak_semantic::semantic_index::DefinitionId;
use oak_semantic::semantic_index::NseScope;
use oak_semantic::semantic_index::NseTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticDiagnostic;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolFlags;
use oak_semantic::semantic_index::UseId;
use oak_semantic::NoopImportsResolver;

use crate::resolvers::TestImportsResolver;

fn index(source: &str) -> SemanticIndex {
    build_with(source, TestImportsResolver::with_base())
}

fn build_with(source: &str, resolver: impl oak_semantic::ImportsResolver) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    build_index(&parsed.tree(), resolver)
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
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
fn test_nse_evalq_explicit_envir_suppresses_body() {
    // `evalq(<expr>, e)` supplies an explicit `envir`, which we don't interpret.
    // The captured expression degrades to a suppressed Quote, so `foo` inside it
    // is NOT recorded as a use. The explicit `e` argument stays a normal use.
    let index = index(
        "\
evalq({
    foo
}, e)
",
    );

    assert!(index.uses_of("foo").is_empty());
    assert!(!index.uses_of("e").is_empty());
}

#[test]
fn test_nse_local_explicit_envir_suppresses_body() {
    // `local(<expr>, e)` supplies an explicit `envir`, so the body degrades to a
    // suppressed Quote: no NSE scope is pushed and `foo` is not a use.
    let index = index(
        "\
local({
    foo
}, e)
",
    );
    let file = ScopeId::from(0);

    // No NSE scope: only the file scope exists.
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(index.scope(file).kind(), ScopeKind::File);
    assert!(index.uses_of("foo").is_empty());
    assert!(!index.uses_of("e").is_empty());
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
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
fn test_nse_ancestor_shadowed_name_no_scope() {
    // A `local` binding in an ENCLOSING scope shadows the base function too,
    // even when the call site sits in a nested scope where `local` is free.
    let index = index(
        "\
local <- function(x) x
f <- function() {
    local({
        y <- 1
    })
}
",
    );
    let file = ScopeId::from(0);
    let identity_fn = ScopeId::from(1);
    let f_scope = ScopeId::from(2);

    // Only three scopes: no NSE scope is pushed for the shadowed `local()`.
    assert_eq!(index.scope_ids().count(), 3);
    assert_eq!(index.scope(file).kind(), ScopeKind::File);
    assert_eq!(index.scope(identity_fn).kind(), ScopeKind::Function);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    // `local` is bound at file scope.
    assert!(index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));

    // `y` is defined flat in `f`, not moved into an NSE child scope.
    assert_eq!(
        index.symbols(f_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_forward_def_visible_to_nested_function() {
    // A function defined inside an eager NSE body that references a name bound
    // LATER in that same body must still resolve to the NSE scope. This relies
    // on the NSE scope's own pre-scan seeing the forward definition, which the
    // pre-scan must collect despite the body range being a Nested NSE range.
    let index = index(
        "\
local({
    f <- function() x
    x <- 1
})
",
    );
    let local_scope = ScopeId::from(1);
    let f_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    // `x` inside `f` resolves to the `local` scope (not the file scope), and
    // its lazy snapshot picks up `x <- 1` (DefinitionId 1 in the local scope:
    // `f` is DefinitionId 0, `x` is DefinitionId 1).
    let x_sym = index.uses(f_scope)[UseId::from(0)].symbol();
    let (enclosing_scope, bindings) = index.enclosing_bindings(f_scope, x_sym).unwrap();
    assert_eq!(enclosing_scope, local_scope);
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
}

#[test]
fn test_nse_moves_definitions_into_nested_scope() {
    // Definitions inside an NSE body land in the NSE child scope, not the
    // parent, even with sibling definitions on either side at file level.
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(
        index.symbols(test_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_block_first_with_trailing_named_arg() {
    // `desc =` is consumed by name, so the leading block fills the remaining
    // `code` formal and gets the Nested + Eager scope. Signature-aware matching
    // (fill remaining, not raw call position) is what recognizes this shape.
    let index = index(
        r#"
testthat::test_that({
    x <- 1
}, desc = "foo")
"#,
    );
    let test_scope = ScopeId::from(1);

    assert_eq!(
        index.scope(test_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(fun_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(fun_scope).parent(), Some(local_scope));
}

#[test]
fn test_nse_prescan_skips_nested_bodies() {
    // The file scope's bound names must NOT include definitions from inside
    // `local()` bodies. This means a function defined AFTER the local() call
    // should not see `x` from inside local via the bound names.
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

    // In `f`, `x` is free and unbound -- no enclosing snapshot should find it
    // in the file scope.
    assert_eq!(
        index.enclosing_bindings(fun_scope, index.uses(fun_scope)[UseId::from(0)].symbol()),
        None
    );
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
        .enclosing_bindings(
            local_scope,
            index.uses(local_scope)[UseId::from(0)].symbol(),
        )
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
    let (_, bindings) = index
        .enclosing_bindings(fun_scope, index.uses(fun_scope)[UseId::from(0)].symbol())
        .unwrap();
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
        ScopeKind::Nse(NseScope::Current, NseTiming::Lazy)
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
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(fun_scope, index.uses(fun_scope)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
}

#[test]
fn test_nse_unmasked_call_via_nested_scope() {
    // Redefining `local` inside a `local()` body doesn't shadow a later
    // `local()` call: the rebind lands in the first body's NSE scope, so it
    // never enters the file's bound names. The scan walks the first `local()`
    // inline, so the second call sees `local` still unbound in the same pass.
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(
        index.scope(second_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
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
fn test_nse_ancestor_unmask_across_function() {
    // The `local <- identity` rebind lives inside the outer `local()` body, so
    // it never enters the file's bound names. The scan of `f`'s body therefore
    // sees base `local` unbound and marks the inner `local()` NSE, cutting its
    // body out of `f`'s bound names in the same pass. So `x <- 1` lands in the
    // inner NSE scope and `g`'s free `x` stays unresolved (the sibling
    // `local()` binds `x` in its own env, invisible to `g`). The old re-walk
    // needed a second iteration to reach this; the scan gets it in one.
    let index = index(
        "\
local({
    local <- identity
})
f <- function() {
    g <- function() x
    local({
        x <- 1
    })
}
",
    );
    let file = ScopeId::from(0);
    let outer_local = ScopeId::from(1);
    let f_scope = ScopeId::from(2);
    let g_scope = ScopeId::from(3);
    let inner_local = ScopeId::from(4);

    assert_eq!(index.scope_ids().count(), 5);
    assert_eq!(
        index.scope(outer_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(outer_local).parent(), Some(file));
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(g_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(g_scope).parent(), Some(f_scope));
    assert_eq!(
        index.scope(inner_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(inner_local).parent(), Some(f_scope));

    // `x <- 1` lands in the inner local scope, not in `f`.
    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(inner_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // `g`'s free `x` resolves to nothing: the sibling `local()` binds `x` in
    // its own scope, not in `f`. Flow-insensitive bound names would wrongly
    // point it at a stray `x` in `f`.
    let g_x = index.uses(g_scope)[UseId::from(0)].symbol();
    assert_eq!(index.enclosing_bindings(g_scope, g_x), None);
}

#[test]
fn test_nse_sibling_branch_flow_precise() {
    // Flow-precise scan across `if`/`else`. `local` is bound only on the
    // consequence path, so on the else path base `local` is still unbound and
    // `local({...})` is NSE. Flow-insensitive bound names would see `local`
    // bound (from the consequence) and miss the NSE call, leaking `y` into the
    // file scope.
    let index = index(
        "\
if (c) local <- identity else local({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);
    let nse_scope = ScopeId::from(1);

    // Only the file scope and the else branch's NSE scope exist.
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(nse_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(nse_scope).parent(), Some(file));

    // `y` lands in the NSE scope, not the file scope.
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(
        index.symbols(nse_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // `local` is bound at file scope (from the consequence branch).
    assert!(index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));
}

#[test]
fn test_nse_eager_lazy_split_on_later_binding() {
    // A later file-level `local <- identity` does NOT shadow `local()` inside the
    // function `f`. `f`'s body is lazy, so its run time relative to the binding
    // is unknown, and `is_locally_bound` reads only `f`'s eager predecessors (the
    // predecessor snapshot, empty here). So the lazy `local()` is optimistically
    // NSE and `x` moves into its own scope. The genuine ambiguity (does `f` run
    // before or after the binding?) is the overturn lint's job, not a shadow.
    //
    // The eager `local()` at file scope is NSE too, but for a determined reason:
    // it runs before the binding, so its flow-precise state has `local` unbound.
    let index = index(
        "\
f <- function() {
    local({
        x <- 1
    })
}
local({
    y <- 1
})
local <- identity
",
    );
    let file = ScopeId::from(0);
    let f_scope = ScopeId::from(1);
    let f_local = ScopeId::from(2);
    let eager_local = ScopeId::from(3);

    // Four scopes: file, `f`, the NSE `local()` in `f`, and the eager `local()`.
    assert_eq!(index.scope_ids().count(), 4);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    // Lazy `local()` in `f` is NSE (later binding is not a predecessor), so `x`
    // moves into its own scope.
    assert_eq!(
        index.scope(f_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(f_local).parent(), Some(f_scope));
    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(f_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // Eager file-level `local()` runs before the binding, so it is NSE and `y`
    // lands in its own scope.
    assert_eq!(
        index.scope(eager_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(eager_local).parent(), Some(file));
    assert!(index.symbols(f_scope).get("y").is_none());
    assert_eq!(
        index.symbols(eager_local).get("y").unwrap().flags(),
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(fun_scope));

    // `x` is free in the local scope, resolves through to the function scope,
    // then to the file scope. The function scope is lazy, so both defs are
    // visible despite `local` being eager.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(
            local_scope,
            index.uses(local_scope)[UseId::from(0)].symbol(),
        )
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(outer_local).parent(), Some(file));
    assert_eq!(
        index.scope(inner_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
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
fn test_nse_eager_super_assignment_visible_to_later_use() {
    // A `<<-` inside an eager NSE body mutates the enclosing binding mid-run,
    // so uses after it must see the `<<-` definition. The eager snapshot is
    // shared across all uses of the free variable, so it accumulates the `<<-`
    // (a safe over-approximation for the earlier use, correct for the later).
    let index = index(
        "\
x <- 1
local({
    x
    x <<- 2
    x
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // The enclosing snapshot for `x` (in the file scope) carries both the
    // initial `x <- 1` (DefinitionId 0) and the `<<-` target (DefinitionId 1).
    let x_sym = index.uses(local_scope)[UseId::from(0)].symbol();
    let (enclosing_scope, bindings) = index.enclosing_bindings(local_scope, x_sym).unwrap();
    assert_eq!(enclosing_scope, file);
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
}

#[test]
fn test_nse_eager_snapshot_absorbs_unrelated_super_assignment() {
    // The eager snapshot keys on the symbol in the enclosing scope, so it
    // can't tell a `<<-` inside the body from one in a function defined after
    // the call. `f`'s `<<-` is recorded on the file scope while its body is
    // walked, firing the eager watcher, so `local()`'s snapshot over-includes
    // it even though it can't reach the already-run body.
    let index = index(
        "\
x <- 1
local({
    x
})
f <- function() {
    x <<- 2
}
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // File-scope defs in allocation order: `x <- 1` (0), then f's `<<-`
    // target (1, recorded before f's own def since `collect_assignment` walks
    // the value side first), then `f` (2). The snapshot absorbs the `<<-`.
    let x_sym = index.uses(local_scope)[UseId::from(0)].symbol();
    let (enclosing_scope, bindings) = index.enclosing_bindings(local_scope, x_sym).unwrap();
    assert_eq!(enclosing_scope, file);
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
}

#[test]
fn test_nse_eager_snapshot_absorbs_unrelated_routed_definition() {
    // `on_load` is `Current + Lazy`, so its `x <- 2` routes to the file scope
    // as a deferred def. That fires the eager watcher, so `local()`'s snapshot
    // over-includes it, even though the routed def can't reach the already-run
    // body.
    let index = index(
        "\
x <- 1
local({
    x
})
rlang::on_load({
    x <- 2
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // File-scope defs: `x <- 1` (0) and the `on_load`-routed `x <- 2` (1).
    let x_sym = index.uses(local_scope)[UseId::from(0)].symbol();
    let (enclosing_scope, bindings) = index.enclosing_bindings(local_scope, x_sym).unwrap();
    assert_eq!(enclosing_scope, file);
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
}

// --- Resolver-driven recognition ---

#[test]
fn test_nse_noop_resolver_bare_local_stays_flat() {
    // Under Noop, `resolve_effects` returns `None`, so a bare `local` isn't
    // recognized as NSE: no scope is pushed and `x` stays at file scope.
    let index = build_with(
        "\
local({
    x <- 1
})
",
        NoopImportsResolver,
    );
    let file = ScopeId::from(0);

    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_noop_resolver_namespaced_local_pushes_scope() {
    // `pkg::fn` resolves through the resolver's default `resolve_qualified_effects`,
    // which reads the static registry. `::` names the package, so there's no
    // shadowing and no cross-file context needed, hence `base::local` is
    // recognized as NSE even under Noop.
    let index = build_with(
        "\
base::local({
    x <- 1
})
",
        NoopImportsResolver,
    );
    let local_scope = ScopeId::from(1);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_front_gate_skips_resolver_for_unannotated_name() {
    // A bare callee whose name no package annotates never reaches the resolver:
    // the `annotates()` gate short-circuits before consultation.
    let resolver = TestImportsResolver::with_base();
    let consultations = resolver.consultations();

    build_with("frobnicate({ x <- 1 })", resolver);

    assert_eq!(consultations.get(), 0);
}

#[test]
fn test_nse_front_gate_consults_resolver_for_annotated_name() {
    // An annotated bare callee does reach the resolver (contrast with the gate
    // test above).
    let resolver = TestImportsResolver::with_base();
    let consultations = resolver.consultations();

    build_with("local({ x <- 1 })", resolver);

    assert!(consultations.get() > 0);
}

// --- source() bindings visible to the scan ---

#[test]
fn test_nse_sourced_name_shadows_base_callee() {
    // A `source()`-injected `local` shadows base `local`, so the later
    // `local({...})` is NOT NSE. The scan binds the sourced names eagerly
    // (source() runs at its position), so the later callee sees the shadow in
    // the same pass, even though the walk injects the Import def later.
    let index = build_with(
        "\
source(\"utils.R\")
local({
    x <- 1
})
",
        TestImportsResolver::with_base().with_source("utils.R", &["local"]),
    );
    let file = ScopeId::from(0);

    // No NSE scope: the sourced `local` shadows base, so `x` stays flat.
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_sourced_file_without_name_leaves_callee_nse() {
    // Same shape, but the sourced file does not define `local`, so base
    // `local` is unshadowed and `local({...})` IS NSE.
    let index = build_with(
        "\
source(\"utils.R\")
local({
    x <- 1
})
",
        TestImportsResolver::with_base().with_source("utils.R", &["other"]),
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_assign_shadows_base_callee_eager() {
    // `assign("local", identity)` binds `local`, so the later `local({...})` is
    // NOT NSE. The scan records the assign-created binding in flow order, so the
    // later callee sees the shadow in the same pass.
    let index = index(
        "\
assign(\"local\", identity)
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    // No NSE scope: `local` is shadowed, so `x` lands at file scope.
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_assign_shadows_base_callee_in_lazy_body() {
    // The file-scope `assign("local", ...)` must be visible to the lazy shadow
    // check when `f`'s deferred body resolves `local`. The file scan completes
    // before the walk enters `f`, so `bound_names[file]` already carries
    // `local` and the callee is correctly treated as shadowed (not NSE).
    let index = index(
        "\
assign(\"local\", identity)
f <- function() local({
    x <- 1
})
",
    );

    // Scopes: file(0) and f(1) only. A phantom NSE scope inside `f` would be a
    // third.
    assert_eq!(index.scope_ids().count(), 2);
    let f_scope = ScopeId::from(1);
    assert_eq!(
        index.symbols(f_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

// --- Current + Lazy owner bindings visible before the walk ---

#[test]
fn test_nse_on_load_binding_order_independent() {
    // A `local` bound inside a `Current + Lazy` `on_load` body is deferred
    // (lazy-provenance), so it is not a precise predecessor for the lazy `local()`
    // in a sibling function `f`. Both bodies run in an order the engine can't
    // know, so whether the shadow holds when `f` runs is undetermined. The
    // predecessor snapshot excludes the deferred `local`, so `f`'s `local()` is
    // optimistically NSE in both orderings (the overturn lint, pending, flags the
    // ambiguity). `x` moves into its own scope regardless of order.
    let first = index(
        "\
f <- function() local({ x <- 1 })
rlang::on_load({ local <- identity })
",
    );
    // Walk order: file, f, f's `local()` scope, on_load.
    let f_first = ScopeId::from(1);
    let f_local_first = ScopeId::from(2);
    assert_eq!(first.scope_ids().count(), 4);
    assert_eq!(first.scope(f_first).kind(), ScopeKind::Function);
    assert_eq!(
        first.scope(f_local_first).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(first.scope(f_local_first).parent(), Some(f_first));
    assert!(first.symbols(f_first).get("x").is_none());
    assert_eq!(
        first.symbols(f_local_first).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    let second = index(
        "\
rlang::on_load({ local <- identity })
f <- function() local({ x <- 1 })
",
    );
    // Walk order: file, on_load, f, f's `local()` scope.
    let f_second = ScopeId::from(2);
    let f_local_second = ScopeId::from(3);
    assert_eq!(second.scope_ids().count(), 4);
    assert_eq!(second.scope(f_second).kind(), ScopeKind::Function);
    assert_eq!(
        second.scope(f_local_second).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(second.scope(f_local_second).parent(), Some(f_second));
    assert!(second.symbols(f_second).get("x").is_none());
    assert_eq!(
        second.symbols(f_local_second).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_on_load_nested_binding_order_independent() {
    // Same as the direct-binding case above, but the shadowing binding is buried
    // in a nested transparent call (`evalq(...)`). It makes no difference to the
    // NSE decision: the predecessor snapshot reads only `f`'s eager predecessors,
    // and `on_load`'s deferred `local` is not one of them however it is written.
    // So `f`'s `local()` is optimistically NSE in both orderings and `x` moves
    // into its own scope. (Under the old whole-scope read this case was
    // order-dependent, because it hinged on whether the walk had routed `local`
    // to the owner's bound names before it reached `f`.)

    // `f` before the `on_load`.
    let first = index(
        "\
f <- function() local({ x <- 1 })
rlang::on_load({ evalq(local <- identity) })
",
    );
    let f = ScopeId::from(1);
    let nested = ScopeId::from(2);
    assert_eq!(first.scope_ids().count(), 4);
    assert_eq!(first.scope(f).kind(), ScopeKind::Function);
    assert_eq!(
        first.scope(nested).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(first.scope(nested).parent(), Some(f));
    assert!(first.symbols(f).get("x").is_none());
    assert_eq!(
        first.symbols(nested).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // `f` after the `on_load`: same result, `local()` is still NSE.
    let second = index(
        "\
rlang::on_load({ evalq(local <- identity) })
f <- function() local({ x <- 1 })
",
    );
    let f_second = ScopeId::from(2);
    let nested_second = ScopeId::from(3);
    assert_eq!(second.scope_ids().count(), 4);
    assert_eq!(second.scope(f_second).kind(), ScopeKind::Function);
    assert_eq!(
        second.scope(nested_second).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(second.scope(nested_second).parent(), Some(f_second));
    assert!(second.symbols(f_second).get("x").is_none());
    assert_eq!(
        second.symbols(nested_second).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_on_load_deferred_binding_unbound_at_eager_position() {
    // The eager stance: `on_load`'s `local` reaches only the owner's bound names,
    // never `bound_so_far`. A file-level (eager) `local()` after the
    // `on_load` runs before the deferred body, so it treats `local` as unbound
    // and IS NSE. Contrast with the lazy sibling case above.
    let index = index(
        "\
rlang::on_load({ local <- identity })
local({ x <- 1 })
",
    );
    let file = ScopeId::from(0);
    let on_load_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert_eq!(index.scope_ids().count(), 3);
    assert_eq!(
        index.scope(on_load_scope).kind(),
        ScopeKind::Nse(NseScope::Current, NseTiming::Lazy)
    );
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

// --- NSE calls in parameter defaults ---

#[test]
fn test_nse_parameter_default_pushes_scope() {
    // An NSE call in a parameter default is recognized and pushes its scope.
    let index = index("f <- function(a = local({ x <- 1 })) a\n");
    let f_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(f_scope));

    // `x` lands in the default's NSE scope, not the function scope.
    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_parameter_default_shadowed_by_param() {
    // All formals bind at once, so a `local` parameter shadows base `local` in
    // a later default, regardless of order: `local({...})` is NOT NSE and `x`
    // stays flat in the function scope.
    let index = index("f <- function(local, a = local({ x <- 1 })) a\n");
    let f_scope = ScopeId::from(1);

    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.symbols(f_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

// --- Lazy shadow ambiguity diagnostics ---

#[test]
fn test_diagnostic_lazy_shadow_later_eager_binding() {
    // `f`'s `local()` is optimistically NSE, but a later file-level `local`
    // binding could shadow it depending on when `f` runs. Flagged.
    let source = "\
f <- function() local({ x <- 1 })
local <- identity
";
    let index = index(source);

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::LazyShadowAmbiguity { name, range } => {
            assert_eq!(name, "local");
            let start = u32::from(range.start()) as usize;
            let end = u32::from(range.end()) as usize;
            assert_eq!(&source[start..end], "local({ x <- 1 })");
        },
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

#[test]
fn test_diagnostic_lazy_shadow_on_load_binding() {
    // A deferred `on_load` binding of `local` and a lazy sibling's `local()`
    // run in an unknown order. Flagged.
    let index = index(
        "\
f <- function() local({ x <- 1 })
rlang::on_load({ local <- identity })
",
    );

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::LazyShadowAmbiguity { name, .. } => assert_eq!(name, "local"),
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

#[test]
fn test_diagnostic_none_at_eager_position() {
    // The file-level `local()` runs before the `on_load` hook fires, so its
    // "unbound" reading is determined, not a guess. No diagnostic.
    let index = index(
        "\
rlang::on_load({ local <- identity })
local({ x <- 1 })
",
    );
    assert!(index.diagnostics().is_empty());
}

#[test]
fn test_diagnostic_none_when_callee_unbound_everywhere() {
    // `local` is never bound anywhere, so the NSE decision is certain and
    // nothing competes with it. No diagnostic.
    let index = index(
        "\
x <- 1
f <- function() local({ x })
",
    );
    assert!(index.diagnostics().is_empty());
}

#[test]
fn test_diagnostic_none_with_eager_predecessor() {
    // `local` is bound before `f` is defined, a sure shadow, so `f`'s `local()`
    // is not NSE at all. No diagnostic.
    let index = index(
        "\
local <- identity
f <- function() local({ x })
",
    );
    assert!(index.diagnostics().is_empty());
}

// --- Eager linear scan: descent and pending names ---

#[test]
fn test_nse_descent_consults_each_call_once() {
    // The inner `local` sits inside the outer `local`'s eager body. The descent
    // scans it once and the walk installs the pending names instead of
    // re-scanning, so each of the two calls reaches the resolver exactly once.
    let resolver = TestImportsResolver::with_base();
    let consultations = resolver.consultations();

    build_with("local({ local({ x <- 1 }) })", resolver);

    assert_eq!(consultations.get(), 2);
}

#[test]
fn test_nse_descent_current_lazy_owner_routes_to_descent_top() {
    // A `Current + Lazy` body (`on_load`) inside an eager `local` body binds `x`.
    // During the descent, `record_owner_name` must route `x` to the descent top
    // (local), not to the current scope. `scan_lazy_owner_bindings` runs while
    // the arena's `current_scope` is still the file, so only the descent-top
    // shortcut lands `x` in local's pending names.
    //
    // We pin it through a FORWARD reference: `f` uses `x` before `on_load` binds
    // it, so the walk resolves the use through local's `bound_names` (the pending
    // set), not through an already-recorded definition. If the routing regressed,
    // `x` would land in the file and the use would resolve to the file scope.
    let index = index(
        "\
local({
    f <- function() x
    rlang::on_load({ x <- 1 })
})
",
    );
    let local_scope = ScopeId::from(1);
    let f_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    let x_sym = index.uses(f_scope)[UseId::from(0)].symbol();
    let (enclosing_scope, _bindings) = index.enclosing_bindings(f_scope, x_sym).unwrap();
    assert_eq!(enclosing_scope, local_scope);
}

#[test]
fn test_nse_descent_snapshot_through_pending_scope() {
    // The descent records `y` as pending for `local`'s scope; the walk installs
    // it before walking `f`, so `f`'s use of `y` resolves to the enclosing
    // snapshot in `local`.
    let index = index(
        "\
local({
    y <- 1
    f <- function() y
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);
    let f_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(f_scope).parent(), Some(local_scope));

    // `y` lands in local's scope.
    assert_eq!(
        index.symbols(local_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // `f`'s use of `y` resolves to local's snapshot. In local, `y` is
    // DefinitionId 0 (`f` is DefinitionId 1).
    let y_sym = index.uses(f_scope)[UseId::from(0)].symbol();
    let (enclosing_scope, bindings) = index.enclosing_bindings(f_scope, y_sym).unwrap();
    assert_eq!(enclosing_scope, local_scope);
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
}

#[test]
fn test_nse_descent_eager_under_lazy() {
    // `local` resolves during `f`'s walk-time scan (unit = `f`), which descends
    // into the body and records its names as pending. `x` lands in local's
    // Nested+Eager scope, not in `f`.
    let index = index(
        "\
f <- function() {
    local({
        x <- 1
    })
}
",
    );
    let f_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(f_scope));

    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_descent_nested_eager_in_eager() {
    // `local({ local({ y <- 1 }) })`: descent stack depth 2, each body's names
    // pending under its own range. `y` lands in the inner scope.
    let index = index(
        "\
local({
    local({
        y <- 1
    })
})
",
    );
    let file = ScopeId::from(0);
    let outer_local = ScopeId::from(1);
    let inner_local = ScopeId::from(2);

    assert_eq!(index.scope_ids().count(), 3);
    assert_eq!(
        index.scope(outer_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(outer_local).parent(), Some(file));
    assert_eq!(
        index.scope(inner_local).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(index.scope(inner_local).parent(), Some(outer_local));

    assert!(index.symbols(outer_local).get("y").is_none());
    assert_eq!(
        index.symbols(inner_local).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_descent_lazy_flag_eager_vs_lazy_context() {
    // An eager callee at file scope consults with `lazy = false`; the same
    // callee inside a function body consults with `lazy = true`.
    let resolver = TestImportsResolver::with_base();
    let log = resolver.consultation_log();

    build_with(
        "\
local({ x <- 1 })
f <- function() {
    local({ y <- 1 })
}
",
        resolver,
    );

    let records = log.borrow();
    let local_lazy: Vec<bool> = records
        .iter()
        .filter(|(name, _lazy)| name == "local")
        .map(|(_name, lazy)| *lazy)
        .collect();
    assert_eq!(local_lazy, vec![false, true]);
}

#[test]
fn test_nse_descent_eager_in_eager_in_function_stays_lazy() {
    // An eager `local` nested inside another eager `local` inside a function
    // still consults with `lazy = true`. Laziness is a property of the enclosing
    // scan unit (the function), which the descent preserves by keeping
    // `current_scope` on the function while it scans both eager bodies inline. If
    // the inner `local` were resolved against its immediate eager scope instead,
    // `is_lazy()` would read `false` and the flag would regress.
    let resolver = TestImportsResolver::with_base();
    let log = resolver.consultation_log();

    build_with(
        "\
f <- function() {
    local({
        local({ x <- 1 })
    })
}
",
        resolver,
    );

    let records = log.borrow();
    let local_lazy: Vec<bool> = records
        .iter()
        .filter(|(name, _lazy)| name == "local")
        .map(|(_name, lazy)| *lazy)
        .collect();
    assert_eq!(local_lazy, vec![true, true]);
}

// --- Attach tracking ---

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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
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
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_nse_attach_eager_body_inside_lazy_body_is_deferred() {
    // `local` is eager, but it sits inside `f`'s function body, so reaching
    // its `library(shiny)` waits on `f()` being called. The attach is recorded
    // (at `local`'s eager scope) but does not run at the file's top level: the
    // eager `attached_packages()` must exclude it, only `_anywhere()` sees it.
    // Guards against a one-scope `is_lazy()` check that misses the lazy
    // ancestor.
    let index = index(
        "\
f <- function() {
    local({
        library(shiny)
    })
}
",
    );
    let f_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert!(index.attached_packages().is_empty());
    assert_eq!(index.attached_packages_anywhere(), vec!["shiny"]);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
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

#[test]
fn test_nse_attach_recognition_respects_shadowing() {
    // `library` is rebound before the attach call, so `library(shiny)` isn't an
    // attach: shiny never attaches and the later `reactive` is not NSE. The bug
    // this fixes: a syntactic `fn_name == "library"` match recorded a bogus
    // attach here.
    let index = index(
        "\
library <- quote
library(shiny)
reactive({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);

    assert!(index.attached_packages().is_empty());
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_shadow_confined_to_nse_scope() {
    // The `library` rebind is confined to `local`'s scope, so the top-level
    // `library(shiny)` sees the unshadowed `library` and attaches. The descent
    // keeps the rebind from leaking, so `reactive` is NSE.
    let index = index(
        "\
local({
    library <- quote
})
library(shiny)
reactive({
    y <- 1
})
",
    );
    let reactive_scope = ScopeId::from(2);

    assert_eq!(index.attached_packages(), vec!["shiny"]);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_nse_attach_in_body_verb_shadow_determined() {
    // Inside `local`, `library` is rebound before `library(shiny)`, so the
    // descent resolves the rebind first and the attach call is not an attach.
    // shiny never attaches and `reactive` is not NSE. Neither the rebind nor a
    // (non-)attach leaks out of `local`.
    let index = index(
        "\
local({
    library <- quote
    library(shiny)
})
reactive({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);

    assert!(index.attached_packages().is_empty());
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_within_lazy_body_not_yet_supported() {
    // Sequential-within-one-lazy-body: when `f` runs, `library(shiny)` runs
    // before `reactive`, so `reactive` is determinately NSE. We don't promote it
    // today: `attached_flow` only grows in eager context, so the attach inside
    // `f` (a lazy body) isn't visible to `reactive` in the same body. The attach
    // is still recorded as a `SemanticCall::Attach`. This could be supported by
    // tracking a per-unit attach set seeded from the EOF view, parallel to
    // `bound_so_far`; deferred for now.
    let index = index(
        "\
f <- function() {
    library(shiny)
    reactive({
        x <- 1
    })
}
",
    );
    let f_scope = ScopeId::from(1);

    // The attach is recorded (scoped to `f`), but not fed to `reactive`, and
    // not counted at the file's top level: only `attached_packages_anywhere()`
    // sees a `library()` buried in a function body.
    assert_eq!(index.attached_packages_anywhere(), vec!["shiny"]);
    assert!(index.attached_packages().is_empty());
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
}
