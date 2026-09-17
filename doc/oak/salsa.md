# Salsa queries

## Inventory snapshots

`oak_db` and `oak_scan` each run the `salsa_inventory` snapshot test from `oak_tidy`. The snapshot lists the crate's `#[salsa::tracked]` functions, `#[salsa::input]` and `#[salsa::interned]` declarations, and tracked structs. It includes their keys, options, and `cycle_result` handlers.

The test catches changes to this API surface, but not changes to query dependencies. A query body or an ordinary helper it calls can introduce a cycle without changing the snapshot. Review such changes even when the snapshot does not change, and treat a snapshot failure as a prompt to review the changed query before accepting the new snapshot.

## Cycle handlers

A cycle occurs when evaluation reaches a query with the same key while that query is already active. Salsa calls the second occurrence the repeated key. The repeated query needs an appropriate `cycle_result` fallback, often an empty value.

Whether a query is repeated depends on where evaluation starts. Check every production operation that can reach the query, even after finding a cycle that repeats another query.

Every `oak_db` cycle handler must call `recovery::record()` with the query key before producing its fallback. Recovery often returns an ordinary empty value that is indistinguishable from successful evaluation, so this hook is how tests and the fuzz harness observe which handler fired. When adding or changing a handler, update `Recovery` and its rendering alongside the handler signature.

## Reviewing a query or dependency change

A query's body does not show every query it can reach. For example, `File::semantic_index()` calls `build_semantic_index()`, which eventually reaches `exports()` through `SalsaImportsResolver` and `oak_semantic`. `File::exports()` calls `semantic_index()` directly.

1.  Follow calls through ordinary helpers, not only Salsa queries. Record which database capability each helper accepts. A helper accepting only `&dyn SourceDb` cannot call `Db`-only semantic queries through that parameter. Recovery fallbacks rely on this restriction, so review `SourceDb` methods and their transitive dependencies for semantic-query calls, and do not widen fallback parameters to `&dyn Db`. `ResolverDb` is the deliberate exception: it wraps the full `Db` but exposes only three cycle-aware forwarding methods. Review each forwarding method's transitive dependencies when changing it. Check `build_semantic_index()`, the collation helpers in `file_imports.rs`, `SalsaImportsResolver` methods in `imports.rs`, `ResolverDb` methods in `resolver_db.rs`, and the `DbInputs`, `SourceDb`, and `Db` implementations in `storage.rs`.
2.  List every production operation that can start a path to the query. Check diagnostics, file resolution, `resolve_at()`, `imports_at()`, package resolution, and workspace aggregates.
3.  From each starting operation, trace whether execution can return to the same query key.
4.  Check every starting operation, even after finding a cycle through another query. The query under review needs a handler if any path can return to it.
5.  Record the starting operations you checked and any path you could not trace to the end. An incomplete path is a gap in the analysis, not evidence that no cycle exists.

## Known recursive paths

This list is not exhaustive. Analyze any recursion introduced by the changed query.

- `semantic_index()`, `exports()`, `attached_packages()`, and `cross_file_layers()` can call back into each other while resolving `source()` and attach effects.
- `source()` site resolution and attach-effect resolution can re-enter those queries.
- `cross_file_layers()` can re-enter while resolving a collation predecessor's `source()` call. Its cycle handler omits predecessor attaches when `cross_file_layers()` is the repeated key.
- `Package::resolve()` can recurse through NAMESPACE re-exports.

When a query changes, state whether it can participate in a cycle, why, and which production paths were examined.
