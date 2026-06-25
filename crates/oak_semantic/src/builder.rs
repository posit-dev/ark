use std::sync::Arc;

use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::AnyRValue;
use aether_syntax::RArgumentList;
use aether_syntax::RBinaryExpression;
use aether_syntax::RExpressionList;
use aether_syntax::RFunctionDefinition;
use aether_syntax::RNamespaceExpression;
use aether_syntax::RParameter;
use aether_syntax::RParameters;
use aether_syntax::RRoot;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstPtr;
use biome_rowan::AstSeparatedList;
use biome_rowan::SyntaxNodeCast;
use biome_rowan::TextRange;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::AnyRSelectorExt;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use oak_index_vec::Idx;
use oak_index_vec::IndexVec;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::resolver::ImportsResolver;
use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionId;
use crate::semantic_index::DefinitionKind;
use crate::semantic_index::EnclosingSnapshotId;
use crate::semantic_index::EnclosingSnapshotKey;
use crate::semantic_index::NamespaceAccess;
use crate::semantic_index::NamespaceAccessKind;
use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;
use crate::semantic_index::Scope;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticCall;
use crate::semantic_index::SemanticCallKind;
use crate::semantic_index::SemanticIndex;
use crate::semantic_index::SymbolFlags;
use crate::semantic_index::SymbolTableBuilder;
use crate::semantic_index::Use;
use crate::semantic_index::UseId;
use crate::use_def_map::UseDefMapBuilder;

mod builder_nse;

/// Build a [`SemanticIndex`] from a parsed R file with cross-file
/// information supplied by `resolver`. See [`ImportsResolver`] for the
/// available impls.
///
/// NSE scopes (`local()`, `test_that()`, ...) require a two-phase build.
/// The first walk keeps everything flat and discovers which calls are NSE.
/// If none are found, that result is final. Otherwise we re-walk with known
/// nested NSE scope bodies.
pub fn build_index(root: &RRoot, resolver: impl ImportsResolver) -> SemanticIndex {
    let range = root.syntax().text_trimmed_range();

    // First walk: discover which calls are NSE, if any.
    let mut builder = SemanticIndexBuilder::new(range, resolver);
    builder.pre_scan_scope(root.syntax());
    builder.collect_expression_list(&root.expressions());

    if !builder.found_nse {
        return builder.finish();
    }

    // Re-walk until the set of NSE scope bodies stabilizes. One re-walk is
    // typically enough to reach convergence. More walks are needed only when
    // pushing an NSE scope unmasks a callee. For instance in:
    //
    // ```
    // local({ with <- identity });
    // with(df, y)
    // ```
    //
    // The call to `with()` is recognized only once a re-walk has moved the
    // `with` assignment into the `local()` scope. Each such level costs one
    // extra walk. Convergence relies on decisions never flipping back from NSE
    // to not-NSE, see `is_locally_bound()`.
    //
    // The loop terminates on its own because the set can only grow. Each
    // re-walk is seeded with the previous set and only inserts. The cap only
    // guards against pathological files.
    //
    // An important caveat is that each walk re-analyzes the whole file, so our
    // passes never get cheaper, which is fine since rewalks should be rare.
    // That's the opposite of Rust-Analyzer's fixpoint, where each pass touches
    // only the shrinking unresolved frontier, which is why RA can afford a much
    // larger cap of 8192 passes:
    // https://github.com/rust-lang/rust-analyzer/blob/abb1301c/crates/hir-def/src/nameres/collector.rs#L61
    const MAX_NSE_ITERATIONS: usize = 64;
    for i in 0..MAX_NSE_ITERATIONS {
        let prev_ranges = std::mem::take(&mut builder.nse_nested_ranges);
        let resolver = builder.resolver;
        builder = SemanticIndexBuilder::new_rewalk(range, prev_ranges.clone(), resolver);
        builder.pre_scan_scope(root.syntax());
        builder.collect_expression_list(&root.expressions());

        if builder.nse_nested_ranges == prev_ranges {
            if i >= 5 {
                log::trace!("NSE re-walk converged after {i} iterations in range {range:?}");
            }
            return builder.finish();
        }
    }

    // Hitting the cap means the returned index is inconsistent, not merely
    // degraded, and valid R should never reach it. `error!` matches that.
    log::error!(
        "NSE re-walk did not converge after {MAX_NSE_ITERATIONS} iterations in range {range:?}"
    );
    builder.finish()
}

// Maintains the preorder allocation invariant on `Scope::descendants`. The
// parallel arrays are pushed in lockstep so they stay indexed by the same
// `ScopeId`.
struct SemanticIndexBuilder<R: ImportsResolver> {
    scopes: IndexVec<ScopeId, Scope>,
    symbol_tables: IndexVec<ScopeId, SymbolTableBuilder>,
    definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
    uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
    use_def_maps: IndexVec<ScopeId, UseDefMapBuilder>,
    current_scope: ScopeId,
    pre_scans: IndexVec<ScopeId, PreScanScope>,
    enclosing_snapshots: FxHashMap<EnclosingSnapshotKey, (ScopeId, EnclosingSnapshotId)>,
    semantic_calls: Vec<SemanticCall>,
    namespace_accesses: Vec<NamespaceAccess>,
    // The `Nested` NSE scope bodies found so far, as a set of ranges. This is
    // the re-walk loop's fixpoint state. Each walk seeds it from the previous
    // iteration, grows it as `record_nse_arg_decision()` recognizes more scopes,
    // and stops once a walk no longer finds any NSE range.
    nse_nested_ranges: FxHashSet<TextRange>,
    // `true` once any scope-pushing NSE combo is found. Triggers the re-walk.
    found_nse: bool,
    // Whether to push NSE scopes at call sites. Only the re-walk does. On the
    // first walk the pre-scan hasn't learned which bodies to skip, so it still
    // records a nested body's definitions (e.g. `x` from `local({x <- 1})`)
    // into the parent scope. Pushing the child scope on that walk too would
    // then register `x`'s enclosing snapshot against the parent, one scope too
    // high.
    is_rewalk: bool,
    resolver: R,
}

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    fn new(range: TextRange, resolver: R) -> Self {
        Self::new_impl(range, FxHashSet::default(), false, resolver)
    }

    fn new_rewalk(range: TextRange, nse_nested_ranges: FxHashSet<TextRange>, resolver: R) -> Self {
        Self::new_impl(range, nse_nested_ranges, true, resolver)
    }

    fn new_impl(
        range: TextRange,
        nse_nested_ranges: FxHashSet<TextRange>,
        is_rewalk: bool,
        resolver: R,
    ) -> Self {
        let mut scopes = IndexVec::new();
        let mut symbol_tables = IndexVec::new();
        let mut definitions = IndexVec::new();
        let mut uses = IndexVec::new();
        let mut use_def_maps = IndexVec::new();
        let mut pre_scans = IndexVec::new();

        // The descendants range starts empty (`n+1..n+1`). `pop_scope` later
        // fills in `descendants.end` with the current arena length. Everything
        // allocated between push and pop is a descendant.
        let file_scope = scopes.push(Scope {
            parent: None,
            kind: ScopeKind::File,
            range,
            descendants: ScopeId::from(1)..ScopeId::from(1),
        });

        // All `ScopeId`-indexed vecs must be pushed in lockstep so they stay
        // the same length. The `push_scope()` method is in charge of
        // guaranteeing that invariant after construction.
        symbol_tables.push(SymbolTableBuilder::new());
        definitions.push(IndexVec::new());
        uses.push(IndexVec::new());
        use_def_maps.push(UseDefMapBuilder::new());
        pre_scans.push(PreScanScope::new());

        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
            use_def_maps,
            current_scope: file_scope,
            pre_scans,
            enclosing_snapshots: FxHashMap::default(),
            semantic_calls: Vec::new(),
            namespace_accesses: Vec::new(),
            nse_nested_ranges,
            found_nse: false,
            is_rewalk,
            resolver,
        }
    }

    fn push_scope(&mut self, kind: ScopeKind, range: TextRange) -> ScopeId {
        let parent = Some(self.current_scope);
        let next_raw = self.scopes.next_id().index() as u32;

        // Descendants start right after this scope. `end` is later filled in by
        // `pop_scope`.
        let descendants = ScopeId::from(next_raw + 1)..ScopeId::from(next_raw + 1);

        let id = self.scopes.push(Scope {
            parent,
            kind,
            range,
            descendants,
        });
        self.current_scope = id;

        self.symbol_tables.push(SymbolTableBuilder::new());
        self.definitions.push(IndexVec::new());
        self.uses.push(IndexVec::new());
        self.use_def_maps.push(UseDefMapBuilder::new());
        self.pre_scans.push(PreScanScope::new());

        id
    }

    fn pop_scope(&mut self, id: ScopeId) {
        // Close the descendants range: everything allocated from `push_scope()`
        // to here is a descendant.
        self.scopes[id].descendants.end = self.scopes.next_id();
        self.current_scope = match self.scopes[id].parent {
            Some(parent) => parent,
            None => panic!("`pop_scope()` called on the file scope"),
        };
    }

    fn add_definition(
        &mut self,
        name: &str,
        flags: SymbolFlags,
        kind: DefinitionKind,
        range: TextRange,
    ) {
        // `Nse(Current, Lazy)` scopes don't own any definitions. We add the
        // definitions to the real enclosing owner scope. Note that `Current +
        // Eager` never reaches here because it doesn't push a scope.
        if matches!(
            self.scopes[self.current_scope].kind,
            ScopeKind::Nse(NseScope::Current, NseTiming::Lazy)
        ) {
            self.add_definition_to_owner(name, flags, kind, range);
            return;
        }

        let symbol_id = self.symbol_tables[self.current_scope].intern(name, flags);
        let def_id = self.definitions[self.current_scope].push(Definition {
            symbol: symbol_id,
            kind,
            range,
        });
        self.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
        self.use_def_maps[self.current_scope].record_definition(symbol_id, def_id);
    }

    /// Route a definition from a `Current + Lazy` scope to the scope that
    /// owns it. That's the nearest ancestor scope which holds its own
    /// definitions. A chain of `Current + Lazy` scopes (e.g. `on_load()` nested
    /// in `on_load()`) is skipped: each one routes to its own owner, so they
    /// all land in the same outer scope.
    pub(super) fn add_definition_to_owner(
        &mut self,
        name: &str,
        flags: SymbolFlags,
        kind: DefinitionKind,
        range: TextRange,
    ) {
        let Some(target_scope) = self.definition_owner() else {
            stdext::debug_panic!("Current + Lazy scope has no parent");
            return;
        };

        let symbol_id = self.symbol_tables[target_scope].intern(name, flags);
        let def_id = self.definitions[target_scope].push(Definition {
            symbol: symbol_id,
            kind,
            range,
        });

        self.use_def_maps[target_scope].ensure_symbol(symbol_id);

        // Deferred: the body executes at an unknown later time, so the
        // definition shouldn't shadow what's already live. This is the same
        // mechanism as `<<-`.
        //
        // Known imprecision: the deferred def is visible to ALL uses in
        // the parent scope (with `may_be_unbound: true`), including
        // file-level uses that run before the lazy body executes. Ideally
        // these defs would only be reachable from lazy scopes (functions),
        // not from eager/file-level code.
        self.use_def_maps[target_scope].record_deferred_definition(symbol_id, def_id);
    }

    /// The scope that owns definitions of a `Current + Lazy` NSE scope. The
    /// climb is iterative to handle e.g. `on_load(on_load(...))`. Every other
    /// scope kind (`File`, `Function`, `Nse(Nested, _)`) owns its definitions
    /// and stops the climb.
    fn definition_owner(&self) -> Option<ScopeId> {
        let mut scope = self.scopes[self.current_scope].parent?;
        while matches!(
            self.scopes[scope].kind,
            ScopeKind::Nse(NseScope::Current, NseTiming::Lazy)
        ) {
            scope = self.scopes[scope].parent?;
        }
        Some(scope)
    }

    // Super-assignment is lexically in the current scope but binds in an
    // ancestor. We record the definition in the current scope and append
    // it to the target scope's use-def map (without shadowing prior
    // definitions).
    //
    // R's `<<-` walks up the environment chain from the parent, targeting
    // the first scope where the symbol is already bound. If no binding is
    // found, it assigns in the global (file) scope.
    fn add_super_definition(&mut self, name: &str, kind: DefinitionKind, range: TextRange) {
        let Some(parent) = self.scopes[self.current_scope].parent else {
            // A top-level `<<-` has no enclosing frame to walk to, so it binds
            // in the file scope it already sits in. The marker scope and the
            // binding scope coincide, so record one definition carrying both
            // flags rather than pushing two coinciding entries.
            let symbol_id = self.symbol_tables[self.current_scope].intern(
                name,
                SymbolFlags::IS_SUPER_BOUND.union(SymbolFlags::IS_BOUND),
            );
            let def_id = self.definitions[self.current_scope].push(Definition {
                symbol: symbol_id,
                kind,
                range,
            });
            self.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
            self.use_def_maps[self.current_scope].record_deferred_definition(symbol_id, def_id);
            return;
        };

        let target_scope = self.resolve_super_target(name, parent);

        let symbol_id =
            self.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_SUPER_BOUND);
        self.definitions[self.current_scope].push(Definition {
            symbol: symbol_id,
            kind: kind.clone(),
            range,
        });

        let target_symbol = self.symbol_tables[target_scope].intern(name, SymbolFlags::IS_BOUND);
        let target_def_id = self.definitions[target_scope].push(Definition {
            symbol: target_symbol,
            kind,
            range,
        });
        self.use_def_maps[target_scope].ensure_symbol(target_symbol);
        self.use_def_maps[target_scope].record_deferred_definition(target_symbol, target_def_id);
    }

    // Walk up from `start` to the first scope where `name` already has
    // `IS_BOUND`. Returns that scope, or the file scope if no binding is found
    // (mirroring R's assignment to the global environment). Reaching the file
    // scope unbound ends the walk there, so its `parent` of `None` is the
    // natural terminator.
    fn resolve_super_target(&self, name: &str, start: ScopeId) -> ScopeId {
        let mut scope = start;
        loop {
            if let Some(id) = self.symbol_tables[scope].id(name) {
                if self.symbol_tables[scope]
                    .symbol(id)
                    .flags()
                    .contains(SymbolFlags::IS_BOUND)
                {
                    return scope;
                }
            }
            let Some(parent) = self.scopes[scope].parent else {
                return scope;
            };
            scope = parent;
        }
    }

    fn add_use(&mut self, name: &str, range: TextRange) {
        let symbol_id = self.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_USED);
        let use_id = self.uses[self.current_scope].push(Use {
            symbol: symbol_id,
            range,
        });
        self.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
        self.use_def_maps[self.current_scope].record_use(symbol_id, use_id);

        // Associate free variables with the enclosing snapshot where the
        // variable is defined
        if self.use_def_maps[self.current_scope].is_may_be_unbound(symbol_id) {
            let use_key = EnclosingSnapshotKey {
                nested_scope: self.current_scope,
                nested_symbol: symbol_id,
            };
            self.register_enclosing_snapshot(name, use_key);
        }
    }

    fn register_enclosing_snapshot(&mut self, name: &str, use_key: EnclosingSnapshotKey) {
        // We're looking for a parent definition for this scope's free variable
        // so start from parent
        let Some(mut current_scope) = self.scopes[self.current_scope].parent else {
            return;
        };

        // Eager vs lazy snapshot for this free variable. Eager snapshots are
        // precise, lazy ones over-approximate. For instance in:
        //
        // ```
        // x <- 1
        // local({ x })
        // x <- 2
        // ```
        //
        // The eager body in `local()` captures `x <- 1` but not `x <- 2`. If
        // the body was inside a lazy context like `function()` instead, the use
        // of `x` could run at any time and we'd fall back to the accumulated
        // union `{1, 2}`, which is an over-approximation.
        //
        // A precise enclosing snapshot requires eagerness throughout, which we
        // track with `all_eager`.
        let mut all_eager = !self.scopes[self.current_scope].kind.is_lazy();

        loop {
            if self.scope_binds_anywhere(current_scope, name) {
                // Intern with empty flags: we just need a stable `SymbolId` for
                // the lookup key. If the symbol was found via its `IS_BOUND`
                // flag, it already exists. If found via pre-scan only, the later
                // `add_definition()` call during the full walk will set `IS_BOUND`.
                let enclosing_symbol_id =
                    self.symbol_tables[current_scope].intern(name, SymbolFlags::empty());

                if self.enclosing_snapshots.contains_key(&use_key) {
                    return;
                }

                self.use_def_maps[current_scope].ensure_symbol(enclosing_symbol_id);

                let snapshot_id = if all_eager {
                    self.use_def_maps[current_scope].register_eager_snapshot(enclosing_symbol_id)
                } else {
                    self.use_def_maps[current_scope].register_lazy_snapshot(enclosing_symbol_id)
                };
                self.enclosing_snapshots
                    .insert(use_key, (current_scope, snapshot_id));

                return;
            }

            if self.scopes[current_scope].kind.is_lazy() {
                all_eager = false;
            }

            let Some(parent) = self.scopes[current_scope].parent else {
                return;
            };
            current_scope = parent;
        }
    }

    /// Whether `scope` binds `name` in its flow state so far: some definition
    /// reaches this point on the control-flow paths up to here. A name never
    /// interned in `scope` has nothing binding it, so it counts as unbound.
    ///
    /// Used for eager scopes, see
    /// [`scope_binds_anywhere`](Self::scope_binds_anywhere) for the
    /// flow-insensitive variant for lazy scopes.
    fn scope_binds_so_far(&self, scope: ScopeId, name: &str) -> bool {
        match self.symbol_tables[scope].id(name) {
            Some(symbol_id) => !self.use_def_maps[scope].is_unbound(symbol_id),
            None => false,
        }
    }

    /// Whether `scope` binds `name` anywhere, regardless of flow position: an
    /// already-recorded `IS_BOUND` definition or a pre-scanned assignment. The
    /// pre-scan covers definitions the walk hasn't reached yet in this scope.
    ///
    /// Used for lazy scopes, see `scope_binds_so_far` for the flow-sensitive
    /// variant for eager scopes.
    fn scope_binds_anywhere(&self, scope: ScopeId, name: &str) -> bool {
        let found_by_flag = self.symbol_tables[scope].id(name).is_some_and(|sym_id| {
            self.symbol_tables[scope]
                .symbol(sym_id)
                .flags()
                .contains(SymbolFlags::IS_BOUND)
        });
        found_by_flag || self.pre_scans[scope].has_name(name)
    }

    // --- Recursive descent ---

    fn collect_expression_list(&mut self, list: &RExpressionList) {
        for expr in list.iter() {
            self.collect_expression(&expr);
        }
    }

    fn collect_expression(&mut self, expr: &AnyRExpression) {
        match expr {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();
                let range = ident.syntax().text_trimmed_range();
                self.add_use(&name, range);
            },

            AnyRExpression::RDots(dots) => {
                self.add_use("...", dots.syntax().text_trimmed_range());
            },

            AnyRExpression::RDotDotI(ddi) => {
                let name = ddi.syntax().text_trimmed().to_string();
                self.add_use(&name, ddi.syntax().text_trimmed_range());
            },

            AnyRExpression::RFunctionDefinition(func) => {
                self.collect_function(func);
            },

            AnyRExpression::RBracedExpressions(braced) => {
                self.collect_expression_list(&braced.expressions());
            },

            AnyRExpression::RBinaryExpression(bin) => {
                // `<-`, `=`, `->`, `<<-`, and `->>` are assignments when they appear as
                // `RBinaryExpression`. In call arguments, `=` is consumed by
                // the parser into `RArgumentNameClause` instead, so it never
                // reaches here.
                if is_assignment(bin) {
                    self.collect_assignment(bin);
                } else {
                    if let Ok(lhs) = bin.left() {
                        self.collect_expression(&lhs);
                    }
                    if let Ok(rhs) = bin.right() {
                        self.collect_expression(&rhs);
                    }
                }
            },

            // Calls and subsets need explicit handling because argument name
            // clauses contain `RIdentifier` nodes that should not be recorded
            // as uses.
            AnyRExpression::RCall(call) => {
                // Record the callee as a use (a no-op for `pkg::fn`) before
                // resolving NSE. That interns the callee symbol, so
                // `resolve_nse()` can look it up by name and read whether it's
                // bound at this point.
                if let Ok(func) = call.function() {
                    self.collect_expression(&func);
                }

                if let Some(annotation) = self.resolve_nse(call) {
                    self.collect_nse_call(call, annotation)
                } else if let Ok(args) = call.arguments() {
                    self.collect_arguments(&args.items());
                }

                self.collect_semantic_call(call);
            },
            AnyRExpression::RSubset(subset) => {
                if let Ok(object) = subset.function() {
                    self.collect_expression(&object);
                }
                if let Ok(args) = subset.arguments() {
                    self.collect_arguments(&args.items());
                }
            },
            AnyRExpression::RSubset2(subset) => {
                if let Ok(object) = subset.function() {
                    self.collect_expression(&object);
                }
                if let Ok(args) = subset.arguments() {
                    self.collect_arguments(&args.items());
                }
            },

            AnyRExpression::RExtractExpression(extract) => {
                // For `x$name` or `x@slot`, collect the object and skip the member
                if let Ok(object) = extract.left() {
                    self.collect_expression(&object);
                }
            },

            AnyRExpression::RNamespaceExpression(expr) => {
                self.collect_namespace_access(expr);
            },

            AnyRExpression::RForStatement(stmt) => {
                // The for variable is always bound (R sets it to NULL for
                // empty sequences), so its binding is recorded before the
                // snapshot. Assignments inside the body are conditional
                // (body may not execute for empty sequences).
                if let Ok(variable) = stmt.variable() {
                    self.add_definition(
                        &variable.name_text(),
                        SymbolFlags::IS_BOUND,
                        DefinitionKind::ForVariable(AstPtr::new(stmt)),
                        variable.syntax().text_trimmed_range(),
                    );
                }
                if let Ok(sequence) = stmt.sequence() {
                    self.collect_expression(&sequence);
                }

                let pre_loop = self.use_def_maps[self.current_scope].snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.uses[self.current_scope],
                    );
                }

                self.use_def_maps[self.current_scope].merge(pre_loop);
            },

            AnyRExpression::RIfStatement(stmt) => {
                // Condition is always evaluated
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_if = self.use_def_maps[self.current_scope].snapshot();

                // If-body (consequence)
                if let Ok(consequence) = stmt.consequence() {
                    self.collect_expression(&consequence);
                }

                let post_if = self.use_def_maps[self.current_scope].snapshot();
                self.use_def_maps[self.current_scope].restore(pre_if);

                // Else-body (alternative), if present. If absent, the
                // "else path" is just the pre-if state we restored to.
                if let Some(else_clause) = stmt.else_clause() {
                    if let Ok(alternative) = else_clause.alternative() {
                        self.collect_expression(&alternative);
                    }
                }

                // After: definitions from both branches are live
                self.use_def_maps[self.current_scope].merge(post_if);
            },

            AnyRExpression::RWhileStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_loop = self.use_def_maps[self.current_scope].snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.uses[self.current_scope],
                    );
                }

                // Body may not execute
                self.use_def_maps[self.current_scope].merge(pre_loop);
            },

            AnyRExpression::RRepeatStatement(stmt) => {
                // Body always executes at least once, so no merge with pre-loop state.
                if let Ok(body) = stmt.body() {
                    let pre_loop = self.use_def_maps[self.current_scope].snapshot();
                    let first_use = self.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.uses[self.current_scope],
                    );
                }
            },

            AnyRExpression::RBogusExpression(_) => {},

            // Generic fallback: walk over descendant nodes and collect their
            // `AnyRExpression` children, letting `collect_expression`
            // handle their contents. This covers `RUnaryExpression`,
            // `RParenthesizedExpression`, `RReturnExpression`, literals, and
            // any future expression types without needing explicit arms.
            //
            // NOTE: This also means that identifiers and assignments inside
            // quoting constructs (`~`, `quote()`, `bquote()`) are recorded as
            // uses and bindings. Refining this requires special-casing these
            // forms, which we defer as future work.
            //
            // Once quoting is handled, `declare()` and `~declare()` will need
            // explicit treatment: its arguments are quoted (not evaluated) but
            // should still be inspected for directives like `source()`.
            // Currently this works by accident because the generic traversal is
            // transparent to both `declare()` and `~`.
            _ => {
                self.collect_descendants(expr.syntax());
            },
        }
    }

    // Walk descendant nodes of `expr`, collecting the outermost
    // `AnyRExpression` nodes and recursing into them via `collect_expression`.
    // This skips intermediate wrapper nodes (e.g. `RElseClause`) while
    // correctly stopping at expression boundaries.
    fn collect_descendants(&mut self, node: &RSyntaxNode) {
        let mut preorder = node.preorder();

        // Skip the root node itself
        preorder.next();

        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            if let Some(expr) = node.cast::<AnyRExpression>() {
                self.collect_expression(&expr);
                preorder.skip_subtree();
            }
        }
    }

    fn collect_function(&mut self, fun: &RFunctionDefinition) {
        let scope = self.push_scope(ScopeKind::Function, fun.syntax().text_trimmed_range());

        if let Ok(params) = fun.parameters() {
            self.collect_parameters(&params);
        }
        if let Ok(body) = fun.body() {
            self.pre_scan_scope(body.syntax());
            self.collect_expression(&body);
        }

        self.pop_scope(scope);
    }

    /// Pre-scan a scope to collect all definition names (skipping nested
    /// function bodies). Runs before the full walk so that enclosing
    /// snapshot registration can find where free variables are bound,
    /// even when the walk in the parent scope hasn't reached the
    /// definition yet. Must stay in sync with the full walk's definition
    /// handling: any construct that calls `add_definition` should have a
    /// corresponding entry here.
    fn pre_scan_scope(&mut self, root: &RSyntaxNode) {
        let mut preorder = root.preorder();
        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            let is_root = &node == root;
            let Some(expr) = AnyRExpression::cast(node) else {
                continue;
            };

            // On the re-walk, skip nested NSE scope bodies, just like we skip
            // function bodies: their definitions belong to the child scope, not
            // the scope being pre-scanned. The root is the scope being
            // pre-scanned, which may itself be an NSE body, so it's never
            // skipped. `nse_nested_ranges` is empty on the first walk.
            if !is_root &&
                self.nse_nested_ranges
                    .contains(&expr.syntax().text_trimmed_range())
            {
                preorder.skip_subtree();
                continue;
            }

            match &expr {
                AnyRExpression::RFunctionDefinition(_) => {
                    preorder.skip_subtree();
                },
                AnyRExpression::RBinaryExpression(bin)
                    if is_assignment(bin) && !is_super_assignment(bin) =>
                {
                    let right = is_right_assignment(bin);
                    let target = if right { bin.right() } else { bin.left() };
                    if let Ok(target) = target {
                        if let Some((name, _)) = assignment_name(&target) {
                            self.pre_scans[self.current_scope].add(name);
                        }
                    }
                },
                AnyRExpression::RForStatement(stmt) => {
                    if let Ok(variable) = stmt.variable() {
                        self.pre_scans[self.current_scope].add(variable.name_text());
                    }
                },
                _ => {},
            }
        }
    }

    fn collect_parameters(&mut self, params: &RParameters) {
        for param in params.items().iter() {
            let Ok(param) = param else { continue };
            self.collect_parameter(&param);
        }
    }

    fn collect_parameter(&mut self, param: &RParameter) {
        let flags = SymbolFlags::IS_BOUND.union(SymbolFlags::IS_PARAMETER);

        if let Ok(name) = param.name() {
            match &name {
                AnyRParameterName::RIdentifier(ident) => {
                    self.add_definition(
                        &ident.name_text(),
                        flags,
                        DefinitionKind::Parameter(AstPtr::new(param)),
                        ident.syntax().text_trimmed_range(),
                    );
                },
                AnyRParameterName::RDots(dots) => {
                    self.add_definition(
                        "...",
                        flags,
                        DefinitionKind::Parameter(AstPtr::new(param)),
                        dots.syntax().text_trimmed_range(),
                    );
                },
                AnyRParameterName::RDotDotI(ddi) => {
                    self.add_definition(
                        &ddi.syntax().text_trimmed().to_string(),
                        flags,
                        DefinitionKind::Parameter(AstPtr::new(param)),
                        ddi.syntax().text_trimmed_range(),
                    );
                },
            }
        }

        if let Some(default) = param.default() {
            if let Ok(value) = default.value() {
                self.collect_expression(&value);
            }
        }
    }

    fn collect_assignment(&mut self, op: &RBinaryExpression) {
        let right = is_right_assignment(op);
        let super_assign = is_super_assignment(op);

        // Value side first to record uses before the binding. The uses
        // might refer to the same symbol as the new binding, but refer
        // to a different place (previous binding).
        let value = if right { op.left() } else { op.right() };
        if let Ok(value) = value {
            self.collect_expression(&value);
        }

        let target = if right { op.right() } else { op.left() };
        let Ok(target) = target else { return };

        let Some((name, range)) = assignment_name(&target) else {
            // Complex target (`x$foo <- rhs`, `x[1] <- rhs`, etc.) does
            // not represent a binding. We recurse for uses.
            self.collect_expression(&target);
            return;
        };

        if super_assign {
            self.add_super_definition(
                &name,
                DefinitionKind::SuperAssignment(AstPtr::new(op)),
                range,
            );
        } else {
            self.add_definition(
                &name,
                SymbolFlags::IS_BOUND,
                DefinitionKind::Assignment(AstPtr::new(op)),
                range,
            );
        }
    }

    fn collect_arguments(&mut self, args: &RArgumentList) {
        for item in args.iter() {
            let Ok(arg) = item else { continue };
            if let Some(value) = arg.value() {
                self.collect_expression(&value);
            }
        }
    }

    fn collect_namespace_access(&mut self, expr: &RNamespaceExpression) {
        let Ok(operator) = expr.operator() else {
            return;
        };
        let kind = match operator.kind() {
            RSyntaxKind::COLON2 => NamespaceAccessKind::Export,
            RSyntaxKind::COLON3 => NamespaceAccessKind::Internal,
            _ => return,
        };
        let Some(package) = expr
            .left()
            .ok()
            .and_then(|selector| selector.identifier_text())
        else {
            return;
        };
        let Some(symbol) = expr
            .right()
            .ok()
            .and_then(|selector| selector.identifier_text())
        else {
            return;
        };
        let offset = expr.syntax().text_trimmed_range().start();
        self.namespace_accesses
            .push(NamespaceAccess::new(package, symbol, kind, offset));
    }

    fn collect_semantic_call(&mut self, call: &aether_syntax::RCall) {
        let Ok(AnyRExpression::RIdentifier(ident)) = call.function() else {
            return;
        };

        let fn_name = ident.name_text();
        if fn_name == "library" || fn_name == "require" {
            self.collect_attach_call(call);
        } else if fn_name == "source" {
            self.collect_source_call(call);
        }
    }

    // ## `library()` / `require()` scoping
    //
    // In R, `library()` always modifies the global search path regardless
    // of where it's called. Statically, we scope the call to
    // `self.current_scope`: at file scope it's visible everywhere (sequential
    // execution is guaranteed), but inside a function it's only visible
    // within that function and its children, since the function might never
    // be called. Same reasoning as `source()` calls.
    fn collect_attach_call(&mut self, call: &aether_syntax::RCall) {
        let Ok(args) = call.arguments() else {
            return;
        };
        let mut items = args.items().iter();

        // For now, only recognise exactly one unnamed argument. We'll do
        // argument matching later (`character.only` unquoting is another
        // complication).
        let Some(Ok(first_arg)) = items.next() else {
            return;
        };
        if first_arg.name_clause().is_some() || items.next().is_some() {
            return;
        }
        let Some(value) = first_arg.value() else {
            return;
        };

        let pkg_name = match &value {
            AnyRExpression::RIdentifier(ident) => Some(ident.name_text()),
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
            _ => None,
        };
        let Some(pkg_name) = pkg_name else {
            return;
        };

        let call_offset = call.syntax().text_trimmed_range().start();
        self.semantic_calls.push(SemanticCall {
            kind: SemanticCallKind::Attach { package: pkg_name },
            offset: call_offset,
            scope: self.current_scope,
        });
    }

    // ## `source()` resolution
    //
    // `source("file.R")` creates `DefinitionKind::Import` forwarding
    // bindings in the current scope for each top-level name exported by
    // the target file. These participate in the use-def map like normal
    // definitions (shadowing, ordering), but goto-definition chases
    // through them via `resolve_definition` to reach the actual origin.
    //
    // The `local` argument is inspected only to bail: if it's set to
    // something other than TRUE/FALSE (e.g., an environment), the call
    // isn't statically analyzable and we skip it.
    //
    // TODO: In nested scopes, `local = FALSE` technically targets the
    // global environment. We currently inject into the calling scope
    // regardless to keep the sourcing mechanism simple. A future diagnostic
    // should suggest `local = TRUE` in nested contexts.
    fn collect_source_call(&mut self, call: &aether_syntax::RCall) {
        let Ok(args) = call.arguments() else {
            return;
        };

        let mut path: Option<String> = None;
        let mut bail = false;

        for item in args.items().iter() {
            let Ok(arg) = item else { continue };

            if let Some(name_clause) = arg.name_clause() {
                let Ok(AnyRArgumentName::RIdentifier(name_ident)) = name_clause.name() else {
                    continue;
                };
                if name_ident.name_text() == "local" {
                    if let Some(value) = arg.value() {
                        match value {
                            // TRUE/FALSE are fine, we resolve uniformly. For
                            // the FALSE in nested context case, we'll emit a
                            // diagnostic.
                            AnyRExpression::RTrueExpression(_) |
                            AnyRExpression::RFalseExpression(_) => {},
                            // With anything else (environment, non-statically
                            // resolvable expression) is not we need to bail.
                            _ => bail = true,
                        }
                    }
                }
            } else if path.is_none() {
                // First positional argument: the file path
                if let Some(AnyRExpression::AnyRValue(AnyRValue::RStringValue(s))) = arg.value() {
                    path = s.string_text();
                }
            }
        }

        if bail {
            return;
        }

        let Some(path) = path else {
            return;
        };

        let call_offset = call.syntax().text_trimmed_range().start();
        let resolution = self.resolver.resolve_source(&path);

        // Record every `source()` call site, independent of whether the
        // resolution was successful. `resolved` pins the canonical URL when
        // resolution succeeded so reflective queries (diagnostics for
        // unresolved `source()`, file-dependency views) read the outcome
        // without re-resolving.
        self.semantic_calls.push(SemanticCall {
            kind: SemanticCallKind::Source {
                path: path.clone(),
                resolved: resolution.as_ref().map(|r| r.url.clone()),
            },
            offset: call_offset,
            scope: self.current_scope,
        });

        let Some(resolution) = resolution else {
            return;
        };

        let file = resolution.url;

        for name in resolution.names {
            // Empty range: R's `source()` imports names implicitly (unlike
            // Python's `from x import y` where `y` appears in the text).
            // There's no text span to assign to these definitions.
            let range = TextRange::empty(call_offset);

            self.add_definition(
                &name,
                SymbolFlags::IS_BOUND,
                DefinitionKind::Import {
                    call: AstPtr::new(call),
                    file: file.clone(),
                    name: name.clone(),
                },
                range,
            );
        }

        // `library()` calls inside the sourced file attach packages to R's
        // global search path at runtime, the same as a `library()` written
        // here directly would. Emit them as `Attach` semantic calls scoped
        // to this `source()`'s offset so scope-layer composition treats
        // them identically to local `library()` calls.
        for pkg in resolution.packages {
            self.semantic_calls.push(SemanticCall {
                kind: SemanticCallKind::Attach { package: pkg },
                offset: call_offset,
                scope: self.current_scope,
            });
        }
    }

    fn finish(mut self) -> SemanticIndex {
        self.scopes[ScopeId::from(0)].descendants.end = self.scopes.next_id();

        let symbol_tables = self
            .symbol_tables
            .into_iter()
            .map(|b| Arc::new(b.build()))
            .collect();

        // The file scope's exit flow state is the file's exports. Capture it
        // before the builders are consumed below.
        let file_final_bindings = self.use_def_maps[ScopeId::from(0)].final_bindings().clone();

        let use_def_maps: IndexVec<ScopeId, _> = self
            .use_def_maps
            .into_iter()
            .zip(self.uses.iter())
            .map(|(b, (_, uses))| Arc::new(b.finish(uses)))
            .collect();

        SemanticIndex::new(
            self.scopes,
            symbol_tables,
            self.definitions,
            self.uses,
            use_def_maps,
            self.enclosing_snapshots,
            self.semantic_calls,
            self.namespace_accesses,
            file_final_bindings,
        )
    }
}

/// All definitions in a scope, collected before the full walk. Skips nested
/// function bodies (those belong to child scopes). Two consumers:
///
/// - Enclosing snapshots: `has_name()` checks whether a symbol will be
///   defined in an ancestor scope (even when the ancestor's walk hasn't reached
///   that definition yet), so that `register_enclosing_snapshot()` can find the
///   right ancestor for free variables.
/// - NSE resolution: With NSE, each function call potentially pushes a scope
///   (which can be lazy or eager). We need to resolve the called function's
///   semantic during the walk. Inside lazy scopes (e.g. function bodies),
///   `by_name` provides the complete set of parent definitions so that the
///   function can be resolved against all the parent scope's definitions (if NSE
///   semantics don't match across definitions, we pick one and lint). Intra-scope
///   resolution is linear and uses the current `symbol_states` directly instead.
struct PreScanScope {
    by_name: FxHashSet<String>,
}

impl PreScanScope {
    fn new() -> Self {
        Self {
            by_name: FxHashSet::default(),
        }
    }

    fn add(&mut self, name: String) {
        self.by_name.insert(name);
    }

    fn has_name(&self, name: &str) -> bool {
        self.by_name.contains(name)
    }
}

fn is_assignment(bin: &RBinaryExpression) -> bool {
    let Ok(op) = bin.operator() else {
        return false;
    };
    matches!(
        op.kind(),
        RSyntaxKind::ASSIGN |
            RSyntaxKind::EQUAL |
            RSyntaxKind::ASSIGN_RIGHT |
            RSyntaxKind::SUPER_ASSIGN |
            RSyntaxKind::SUPER_ASSIGN_RIGHT
    )
}

fn is_right_assignment(bin: &RBinaryExpression) -> bool {
    let Ok(op) = bin.operator() else {
        return false;
    };
    matches!(
        op.kind(),
        RSyntaxKind::ASSIGN_RIGHT | RSyntaxKind::SUPER_ASSIGN_RIGHT
    )
}

/// Extract the binding name and range from an assignment target expression.
/// Returns `None` for complex targets (`x$foo`, `x[1]`, etc.) that don't
/// represent simple name bindings.
fn assignment_name(target: &AnyRExpression) -> Option<(String, TextRange)> {
    match target {
        AnyRExpression::RIdentifier(ident) => {
            let name = ident.name_text();
            let range = ident.syntax().text_trimmed_range();
            Some((name, range))
        },
        // `"x" <- 1` is equivalent to `x <- 1` in R
        AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            let name = s.string_text()?;
            let range = s.syntax().text_trimmed_range();
            Some((name, range))
        },
        _ => None,
    }
}

fn is_super_assignment(bin: &RBinaryExpression) -> bool {
    let Ok(op) = bin.operator() else {
        return false;
    };
    matches!(
        op.kind(),
        RSyntaxKind::SUPER_ASSIGN | RSyntaxKind::SUPER_ASSIGN_RIGHT
    )
}
