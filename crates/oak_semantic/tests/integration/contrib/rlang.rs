use oak_semantic::semantic_index::DefinitionId;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticDiagnostic;
use oak_semantic::semantic_index::SymbolFlags;
use oak_semantic::semantic_index::UseId;

use crate::common::index_with_attached;
use crate::common::index_with_base as index;
use crate::common::only_assign_def;

// --- `%<~%` binding operator ---

#[test]
fn test_rlang_lazy_assignment_binds_lhs() {
    let index = index_with_attached("x %<~% compute()", &["rlang"]);
    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));
}

// --- `on_load` (Current + Lazy) ---

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
        ScopeKind::Nse(EvalEnv::Current, EvalTiming::Lazy)
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
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
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
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
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
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
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
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
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
        ScopeKind::Nse(EvalEnv::Current, EvalTiming::Lazy)
    );
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
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
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    let (enclosing_scope, _bindings) = index.enclosing_bindings(f_scope, UseId::from(0)).unwrap();
    assert_eq!(enclosing_scope, local_scope);
}
