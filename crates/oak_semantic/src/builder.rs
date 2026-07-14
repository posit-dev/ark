//! Builds the [`SemanticIndex`] for one R file.
//!
//! The builder splits work by "scan unit": the file or a lazy body (a function,
//! a lazy NSE body like `reactive()`). A unit is coarser than a scope. An eager
//! scope nested inside it, like `local({ ... })`, is part of the same scan unit,
//! while a lazy body starts a new one.
//!
//! Each scan unit is built in two passes: a scan, then a walk. The walk is the
//! pass that writes the arenas (scopes, symbols, definitions, uses, use-def
//! maps). It can only write them correctly if it already knows two things about
//! the scope it's in, and neither is knowable at its own cursor:
//!
//! - Which calls are NSE, so it can push the scope for `local({ ... })` inline
//!   as it reaches the call. That turns on whether the callee is shadowed at
//!   that point in the flow.
//!
//! - The complete set of names the scope binds, so it can resolve a nested
//!   scope's free variable to an ancestor binding. A lazy body (a function, a
//!   `reactive()`) can reference a definition the ancestor's own walk hasn't
//!   reached yet. That ancestor lookup is what the walk records as an enclosing
//!   snapshot.
//!
//! So there are two flow states, on purpose. The scan's flow state tracks only
//! eager bindings and is allowed to stay coarse (across `if` branches it
//! over-approximates to "bound on some path"). The walk builds the precise
//! structures, such as the use-def map.

use std::sync::Arc;

use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::AnyRValue;
use aether_syntax::RArgumentList;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
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

use crate::effects::AssignBinding;
use crate::effects::ResolvedArgumentEffects;
use crate::resolver::ImportsResolver;
use crate::resolver::SourceResolution;
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
use crate::semantic_index::SemanticDiagnostic;
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
/// See the module docs for the scan/walk split. The scan
/// ([`scan_expression`]) runs first over each scope, then the walk
/// ([`collect_expression`]) reuses its decisions and pushes NSE scopes inline.
///
/// [`scan_expression`]: SemanticIndexBuilder::scan_expression
/// [`collect_expression`]: SemanticIndexBuilder::collect_expression
pub fn build_index(root: &RRoot, resolver: impl ImportsResolver) -> SemanticIndex {
    let range = root.syntax().text_trimmed_range();

    let mut builder = SemanticIndexBuilder::new(range, resolver);
    builder.begin_scan();
    builder.scan_expression_list(&root.expressions());
    builder.collect_expression_list(&root.expressions());
    builder.finish()
}

// Maintains the preorder allocation invariant on `Scope::descendants`. The
// parallel arrays are pushed in lockstep so they stay indexed by the same
// `ScopeId`.
struct SemanticIndexBuilder<R: ImportsResolver> {
    resolver: R,
    scopes: IndexVec<ScopeId, Scope>,
    symbol_tables: IndexVec<ScopeId, SymbolTableBuilder>,
    definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
    uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
    use_def_maps: IndexVec<ScopeId, UseDefMapBuilder>,
    current_scope: ScopeId,
    bound_names: IndexVec<ScopeId, BoundNames>,
    enclosing_snapshots: FxHashMap<EnclosingSnapshotKey, (ScopeId, EnclosingSnapshotId)>,
    semantic_calls: Vec<SemanticCall>,
    namespace_accesses: Vec<NamespaceAccess>,
    // Per-call facts resolved by the scanner in flow order, keyed by the call's
    // range. See `CallResolution`.
    call_resolutions: FxHashMap<TextRange, CallResolution>,
    // Diagnostics collected during the build and logged on `finish()`. A minimal
    // channel for now, no user-facing surface.
    diagnostics: Vec<SemanticDiagnostic>,
    // The scan's flow-precise binding state for the scope being scanned, reset
    // at each scope's `begin_scan()`. See [`FlowState`].
    flow_state: FlowState,
    // Names inherited from enclosing scopes at this scope's entry point, keyed
    // by the scope's range. Captured from `flow_state`, and read by
    // `begin_scan()` to seed the scope's own scan.
    enclosing_flow: FxHashMap<TextRange, FlowState>,
    // Packages attached in eager flow order (file level and eager NSE descents),
    // appended only when `!is_lazy()`. Append-only, never restored across a
    // descent or branch: attaches hit the global search path, they aren't scoped
    // like `flow_state`. An eager callee reads the flow-precise prefix during
    // the file scan. A lazy callee reads the complete set during the walk (which
    // runs after the file scan finishes), so this doubles as the end-of-file
    // attach view.
    attached_flow: Vec<String>,
    // Bound names of Eager + Nested bodies like `local()` are discovered inline
    // by the scanner. See `EagerNestedDescent`.
    eager_descent: EagerNestedDescent,
}

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    fn new(range: TextRange, resolver: R) -> Self {
        let mut scopes = IndexVec::new();
        let mut symbol_tables = IndexVec::new();
        let mut definitions = IndexVec::new();
        let mut uses = IndexVec::new();
        let mut use_def_maps = IndexVec::new();
        let mut bound_names = IndexVec::new();

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
        bound_names.push(BoundNames::new());

        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
            use_def_maps,
            current_scope: file_scope,
            bound_names,
            enclosing_snapshots: FxHashMap::default(),
            semantic_calls: Vec::new(),
            namespace_accesses: Vec::new(),
            call_resolutions: FxHashMap::default(),
            flow_state: FlowState::default(),
            enclosing_flow: FxHashMap::default(),
            attached_flow: Vec::new(),
            eager_descent: EagerNestedDescent::default(),
            diagnostics: Vec::new(),
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
        self.bound_names.push(BoundNames::new());

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

    /// Whether `scope` binds `name` anywhere, regardless of flow position: an
    /// already-recorded `IS_BOUND` definition or a pre-scanned assignment. The
    /// pre-scan covers definitions the walk hasn't reached yet in this scope.
    fn scope_binds_anywhere(&self, scope: ScopeId, name: &str) -> bool {
        let found_by_flag = self.symbol_tables[scope].id(name).is_some_and(|sym_id| {
            self.symbol_tables[scope]
                .symbol(sym_id)
                .flags()
                .contains(SymbolFlags::IS_BOUND)
        });
        found_by_flag || self.bound_names[scope].binds(name)
    }

    /// Record the names a child scope (function body, NSE argument) about to be
    /// created at `range` inherits from its ancestors, to seed the child's scan
    /// in `begin_scan`. Called during the scan, where `flow_state` is the
    /// parent's flow-precise state at the child's definition point (already
    /// carrying the parent's own inherited ancestors, so the child inherits
    /// transitively).
    pub(super) fn record_enclosing_flow(&mut self, range: TextRange) {
        self.enclosing_flow
            .insert(range, self.flow_state.snapshot());
    }

    // --- Scan pass ---

    /// Reset the flow-precise binding state for a fresh scope's scan.
    ///
    /// Seeds it with two things:
    ///
    /// - The names inherited from enclosing scopes, captured when this scope was
    ///   entered (`enclosing_flow`). The parent's own scan was seeded the same
    ///   way, so this is transitively complete: it holds every eager binding
    ///   visible from an ancestor at this scope's definition point.
    /// - The scope's own already-bound names. For a function scope that's the
    ///   parameters, recorded just before the scan runs. For file and NSE scopes
    ///   nothing local is bound yet.
    ///
    /// Parameter defaults are a special case: they are scanned before the params
    /// are recorded, so `collect_function` seeds the full formal set by hand
    /// (all formals bind at once in R, so a default sees every parameter name).
    pub(super) fn begin_scan(&mut self) {
        let range = self.scopes[self.current_scope].range;

        match self.enclosing_flow.get(&range).cloned() {
            Some(entry) => self.flow_state.restore(entry),
            None => self.flow_state.clear(),
        }

        for (_id, symbol) in self.symbol_tables[self.current_scope].iter() {
            if symbol.flags().contains(SymbolFlags::IS_BOUND) {
                self.flow_state.bind(symbol.name().to_string());
            }
        }
    }

    pub(super) fn scan_expression_list(&mut self, list: &RExpressionList) {
        for expr in list.iter() {
            self.scan_expression(&expr);
        }
    }

    /// Scan for NSE calls and collect the scope's bound names, in flow order.
    ///
    /// Runs before the walk of a scope. It decides NSE-ness at each call the
    /// same way the walk's [`is_locally_bound`](Self::is_locally_bound) would,
    /// records the decision in `call_resolutions` for the walk to reuse, and adds
    /// non-skipped definition names to `bound_names`. The bound names must be
    /// complete before the walk descends into any child scope, because a lazy
    /// child body can reference an ancestor def the ancestor's walk hasn't
    /// reached yet.
    ///
    /// A scan unit is the file or a lazy body (function, `Nested + Lazy`,
    /// `Current + Lazy`). Each unit is scanned once. Within a unit the scan
    /// descends through every eager boundary it meets, in flow order:
    ///
    /// - A `Current + Eager` body pushes no scope, so it stays part of this
    ///   scope's direct level and is scanned through transparently.
    /// - A `Nested + Eager` body is descended into with a save/restore of
    ///   `flow_state`, and the names it binds are left pending for the walk to
    ///   install without re-scanning.
    /// - Function and lazy bodies (`Nested + Lazy`, `Current + Lazy`) are their
    ///   own scan units, scanned separately when the walk enters them, because
    ///   NSE resolution there needs the child's own flow context.
    ///
    /// Branch analysis is precise. In `if (c) local <- f else local({ y <- 1
    /// })` the else branch sees an NSE call because `local` is unbound on the
    /// else path, which prevents `y` from leaking into the scope.
    pub(super) fn scan_expression(&mut self, expr: &AnyRExpression) {
        match expr {
            AnyRExpression::RFunctionDefinition(func) => {
                // A function body is a child scope, scanned when it's entered.
                // Record the names it inherits now so that when we later resolve
                // an NSE callee inside the body, we can check whether one of them
                // shadows it (see `enclosing_flow`).
                self.record_enclosing_flow(func.syntax().text_trimmed_range());
            },

            AnyRExpression::RBracedExpressions(braced) => {
                self.scan_expression_list(&braced.expressions());
            },

            AnyRExpression::RBinaryExpression(bin) => {
                if is_assignment(bin) {
                    let right = is_right_assignment(bin);

                    // Value side first, mirroring `collect_assignment`: it may
                    // hold NSE calls or nested defs that flow before the binding.
                    let value = if right { bin.left() } else { bin.right() };
                    if let Ok(value) = value {
                        self.scan_expression(&value);
                    }

                    let target = if right { bin.right() } else { bin.left() };
                    if let Ok(target) = target {
                        match assignment_name(&target) {
                            // `<<-` binds in an ancestor, not here, so it doesn't
                            // shadow a callee in this scope (matching the walk).
                            Some((name, _)) if !is_super_assignment(bin) => {
                                self.record_binding(name);
                            },
                            Some(_) => {},
                            // Complex target (`x$foo <- v`): no binding, but the
                            // target may hold NSE calls.
                            None => self.scan_expression(&target),
                        }
                    }
                } else {
                    // A binding operator (`x %<>% f()`) binds its left operand.
                    // Scan the operands as uses first, then record the binding,
                    // so a later callee in this scope sees that name shadowed.
                    // Mirrors the value-then-target order of the `is_assignment`
                    // branch.
                    if let Ok(lhs) = bin.left() {
                        self.scan_expression(&lhs);
                    }
                    if let Ok(rhs) = bin.right() {
                        self.scan_expression(&rhs);
                    }
                    self.scan_operator_assign(bin);
                }
            },

            AnyRExpression::RCall(call) => {
                if let Ok(func) = call.function() {
                    self.scan_expression(&func);
                }
                self.scan_call(call);
            },

            AnyRExpression::RForStatement(stmt) => {
                // The for-variable is always bound (R sets it to NULL for empty
                // sequences), so it binds before the body regardless of flow.
                if let Ok(variable) = stmt.variable() {
                    self.record_binding(variable.name_text());
                }
                if let Ok(sequence) = stmt.sequence() {
                    self.scan_expression(&sequence);
                }
                // A loop body only adds bindings (a name bound inside still
                // "reaches" on the ran path), so no restore is needed, unlike
                // the two-branch `if`/`else` below.
                if let Ok(body) = stmt.body() {
                    self.scan_expression(&body);
                }
            },

            AnyRExpression::RIfStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.scan_expression(&condition);
                }

                let pre_if = self.flow_state.snapshot();

                if let Ok(consequence) = stmt.consequence() {
                    self.scan_expression(&consequence);
                }

                let post_if = self.flow_state.snapshot();
                self.flow_state.restore(pre_if);

                if let Some(else_clause) = stmt.else_clause() {
                    if let Ok(alternative) = else_clause.alternative() {
                        self.scan_expression(&alternative);
                    }
                }

                // Both branches' bindings are live afterwards.
                self.flow_state.merge(post_if);
            },

            // `while`/`repeat` loops, subsets, extractions, parentheses, unary
            // ops, and literals: recurse into child expressions. Loops need no
            // flow restore (see the `for` arm). Identifiers and dots are leaves
            // with no bindings or calls, so they fall through to a no-op walk.
            _ => {
                self.scan_descendants(expr.syntax());
            },
        }
    }

    /// Walk descendant nodes of `expr`, scanning the outermost
    /// `AnyRExpression` children. The scan analog of
    /// `collect_descendants`.
    fn scan_descendants(&mut self, node: &RSyntaxNode) {
        let mut preorder = node.preorder();
        preorder.next();

        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            if let Some(expr) = node.cast::<AnyRExpression>() {
                self.scan_expression(&expr);
                preorder.skip_subtree();
            }
        }
    }

    fn scan_parameter_defaults(&mut self, params: &RParameters) {
        // Seed `flow_state` with every parameter names so a callee inside a
        // default value sees the full formal set
        for param in params.items().iter() {
            let Ok(param) = param else { continue };
            let Ok(name) = param.name() else { continue };
            let text = match &name {
                AnyRParameterName::RIdentifier(ident) => ident.name_text(),
                AnyRParameterName::RDots(_) => String::from("..."),
                AnyRParameterName::RDotDotI(ddi) => ddi.syntax().text_trimmed().to_string(),
            };
            self.flow_state.bind(text);
        }

        for param in params.items().iter() {
            let Ok(param) = param else { continue };
            let Some(default) = param.default() else {
                continue;
            };
            if let Ok(value) = default.value() {
                self.scan_expression(&value);
            }
        }
    }

    /// Resolve one sourced `path`, bind the names it brings in, and return its
    /// resolution for the caller to cache.
    ///
    /// The binding is eager: `source()` runs at its position, so the sourced
    /// names are bound afterwards and can shadow a later NSE callee (e.g. a
    /// sourced `local` masking base `local`). Returns `None` when the resolver
    /// can't locate the target.
    ///
    /// [`scan_call`]: Self::scan_call
    fn scan_source_call(&mut self, path: &str) -> Option<SourceResolution> {
        let resolution = self.resolver.resolve_source(path)?;

        for name in &resolution.names {
            self.record_binding(name.clone());
        }

        // A `source()`-forwarded `library()` attaches at this call's flow
        // position, the same as an attach written here directly. Only in eager
        // context, matching `scan_attach_call`'s `!is_lazy()` gate.
        if !self.scopes[self.current_scope].kind.is_lazy() {
            for pkg in &resolution.packages {
                self.attached_flow.push(pkg.clone());
            }
        }

        Some(resolution)
    }

    /// Record a binding in the scan's flow state.
    ///
    /// The flow-precise `flow_state` always learns the name, so a
    /// later callee in this scope sees it shadowed. The bound names only get it
    /// when the current scope owns it. A `Current + Lazy` scope routes its defs
    /// to the owner, so the name is added to the owner's bound names instead, the
    /// same routing `add_definition_to_owner` does during the walk.
    fn record_binding(&mut self, name: String) {
        self.record_owner_name(name.clone());
        self.flow_state.bind(name);
    }

    /// Route a binding NAME into its owner scope's bound names, matching
    /// `add_definition`'s routing. When a descent is open the name goes to the
    /// descent top, which is always an eager `Nested` body scanned inline and so
    /// owns its bindings. Otherwise a `Current + Lazy` scope routes to
    /// `definition_owner()` and every other scope owns its bindings.
    ///
    /// Split from `record_binding` so `scan_lazy_owner_bindings` can add
    /// a deferred body's names to the owner's bound names without also marking them
    /// bound in `flow_state` (see that helper for why).
    fn record_owner_name(&mut self, name: String) {
        if let Some(bound) = self.eager_descent.open.last_mut() {
            bound.add(name);
            return;
        }

        if let Some(target) = match self.scopes[self.current_scope].kind {
            ScopeKind::Nse(NseScope::Current, NseTiming::Lazy) => self.definition_owner(),
            _ => Some(self.current_scope),
        } {
            self.bound_names[target].add(name);
        }
    }

    fn nse_effect(&self, call: &RCall) -> Option<ResolvedArgumentEffects> {
        self.call_resolutions
            .get(&call.syntax().text_trimmed_range())
            .and_then(|resolution| resolution.arguments.clone())
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
                    // Operands first, as uses: `x %<>% f` is `x <- f(x)`, so `x`
                    // is both a read and the binding target, and `f` is a use.
                    if let Ok(lhs) = bin.left() {
                        self.collect_expression(&lhs);
                    }
                    if let Ok(rhs) = bin.right() {
                        self.collect_expression(&rhs);
                    }
                    // A `%...%` operator the scan recognized as an assign effect
                    // emits its binding here, after the operand uses.
                    self.collect_assign_operator(bin);
                }
            },

            // Calls and subsets need explicit handling because argument name
            // clauses contain `RIdentifier` nodes that should not be recorded
            // as uses.
            AnyRExpression::RCall(call) => {
                // Record the callee as a use (a no-op for `pkg::fn`) before
                // handling NSE.
                if let Ok(func) = call.function() {
                    self.collect_expression(&func);
                }

                if let Some(scoping) = self.nse_effect(call) {
                    self.collect_nse_call(call, scoping)
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
            // Scan the default values before collecting them. R binds all
            // formals into the frame at once, so a default sees every parameter
            // name regardless of position: `function(local, b = local(...))` is
            // not NSE. So we seed the whole formal set into `flow_state`
            // up front rather than flow-ordered, then scan each default.
            self.begin_scan();
            self.scan_parameter_defaults(&params);

            // `collect_parameters` adds the parameter definitions and walks
            // each default in source order, finding the NSE decisions the scan
            // above recorded.
            self.collect_parameters(&params);
        }
        if let Ok(body) = fun.body() {
            self.begin_scan();
            self.scan_expression(&body);
            self.collect_expression(&body);
        }

        self.pop_scope(scope);
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
        // Attach: the scan recognized it (shadow- and mask-aware) and recorded
        // the package by range. We emit the `SemanticCall::Attach` here so it
        // carries the walk-time scope, e.g. the pushed NSE scope for a
        // `library()` inside `local({...})`.
        let range = call.syntax().text_trimmed_range();
        if let Some(package) = self
            .call_resolutions
            .get(&range)
            .and_then(|resolution| resolution.attach.clone())
        {
            self.record_attach(call, package);
        }

        // Source: the scan recognized it (shadow- and mask-aware) on the resolve
        // path and cached the sourced files by range. Their presence is the
        // recognition marker, so we dispatch on it rather than the callee name.
        if self
            .call_resolutions
            .get(&range)
            .is_some_and(|resolution| !resolution.source.is_empty())
        {
            self.collect_source_call(call);
        }

        // Assign: same recognition path. The scan cached the bound names and we
        // emit the corresponding definitions so they feed the use-def map,
        // `exports()`, and goto.
        if self
            .call_resolutions
            .get(&range)
            .is_some_and(|resolution| !resolution.assign.is_empty())
        {
            self.collect_assign_call(call);
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
    fn record_attach(&mut self, call: &RCall, package: String) {
        let call_offset = call.syntax().text_trimmed_range().start();
        self.semantic_calls.push(SemanticCall {
            kind: SemanticCallKind::Attach { package },
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
        let range = call.syntax().text_trimmed_range();
        let call_offset = range.start();

        // Read back what the scan cached: the sourced files, each with its
        // resolution. The scan is the single point that extracts the paths and
        // consults `resolve_source`, so the walk never re-parses or re-resolves.
        let sourced = match self.call_resolutions.get(&range) {
            Some(resolution) => resolution.source.clone(),
            None => return,
        };

        for SourcedFile { path, resolution } in sourced {
            // Record every sourced file, independent of whether it resolved.
            // `resolved` pins the canonical URL when resolution succeeded so
            // reflective queries (diagnostics for unresolved `source()`,
            // file-dependency views) read the outcome without re-resolving.
            let resolved = resolution.as_ref().map(|r| r.url.clone());
            self.semantic_calls.push(SemanticCall {
                kind: SemanticCallKind::Source { path, resolved },
                offset: call_offset,
                scope: self.current_scope,
            });

            let Some(resolution) = resolution else {
                continue;
            };

            let file = resolution.url;

            for name in resolution.names {
                // Empty range: R's `source()` imports names implicitly (unlike
                // Python's `from x import y` where `y` appears in the text).
                // There's no text span to assign to these definitions.
                let name_range = TextRange::empty(call_offset);

                self.add_definition(
                    &name,
                    SymbolFlags::IS_BOUND,
                    DefinitionKind::Import {
                        call: AstPtr::new(call),
                        file: file.clone(),
                        name: name.clone(),
                    },
                    name_range,
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
    }

    // ## `assign()` binding
    //
    // `assign("x", value)` binds `x` in the current scope, the same as `x <-
    // value` would. We record a `DefinitionKind::Assign` def so it feeds the
    // use-def map, `exports()`, and goto exactly like a syntactic assignment.
    // The name is not chased to its value, so an `assign("f", local)` def
    // carries no NSE, just like `f <- local`.
    fn collect_assign_call(&mut self, call: &aether_syntax::RCall) {
        let range = call.syntax().text_trimmed_range();

        // Read back the bindings the scan extracted (their presence is what the
        // caller checked before dispatching here).
        let bindings = match self.call_resolutions.get(&range) {
            Some(resolution) => resolution.assign.clone(),
            None => return,
        };

        self.add_assign_definitions(&AnyRExpression::RCall(call.clone()), bindings);
    }

    fn add_assign_definitions(&mut self, node: &AnyRExpression, bindings: Vec<AssignBinding>) {
        for binding in bindings {
            // The def's own range is the name token, captured at scan time, so a
            // cursor on the name at the definition site hit-tests to it, the same
            // as a syntactic `<-` binding.
            let name_range = binding.name_expr.text_trimmed_range();
            let name = binding.name_expr.as_ptr().clone();
            self.add_definition(
                &binding.name,
                SymbolFlags::IS_BOUND,
                DefinitionKind::Assign {
                    node: AstPtr::new(node),
                    name,
                    value: binding.value_expr,
                },
                name_range,
            );
        }
    }

    /// Emit the `Assign` definition for a binding operator (e.g. `x %<>% f()`) the
    /// scan recognized, after its operands were collected as uses.
    fn collect_assign_operator(&mut self, bin: &RBinaryExpression) {
        let range = bin.syntax().text_trimmed_range();
        let bindings = match self.call_resolutions.get(&range) {
            Some(resolution) if !resolution.assign.is_empty() => resolution.assign.clone(),
            _ => return,
        };

        self.add_assign_definitions(&AnyRExpression::RBinaryExpression(bin.clone()), bindings);
    }

    fn finish(mut self) -> SemanticIndex {
        self.scopes[ScopeId::from(0)].descendants.end = self.scopes.next_id();

        // TODO(diagnostics): Diagnostics are not surfaced yet, so log them for now
        for diagnostic in &self.diagnostics {
            match diagnostic {
                SemanticDiagnostic::LazyShadowAmbiguity { name, range } => log::warn!(
                    "Lazy-shadow ambiguity: callee `{name}` at {range:?} is recognized \
                     as effectful, but a lazy-crossed ancestor binds it with undetermined timing"
                ),
            }
        }

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
            self.diagnostics,
            file_final_bindings,
        )
    }
}

/// What the scan resolved a single call to, for the walk to reuse. A call can
/// carry several of these at once.
///
/// - `arguments`: the NSE effect the call resolved to, filled in flow order. `None`
///   means "not NSE".
/// - `attach`: the package a `library()`/`require()` call attaches, recognized
///   shadow-aware on the resolve path. The walk reads it back to emit a scoped
///   `SemanticCall::Attach`.
/// - `source`: the files a recognized `source()` call brings in, each with its
///   resolution.
/// - `assign`: the bindings `assign()`-like calls create in the current scope.
#[derive(Default)]
struct CallResolution {
    arguments: Option<ResolvedArgumentEffects>,
    attach: Option<String>,
    source: Vec<SourcedFile>,
    assign: Vec<AssignBinding>,
}

/// A single file a `source()` call brings in: its statically-extracted path and
/// the resolution the scan computed for it (`None` when it didn't resolve).
#[derive(Clone)]
struct SourcedFile {
    path: String,
    resolution: Option<SourceResolution>,
}

/// The scan's flow-precise binding state: which names are bound at the current
/// point of the current scan unit, in flow order.
///
/// It's the scan's own flow state, a coarse variant of the walk's use-def map,
/// which isn't built yet. It answers one question, "is this name bound here?",
/// so the scan can tell whether a callee is shadowed at each call and decide
/// whether a call is NSE. It tracks only eager bindings, and it is allowed to
/// stay coarse: `merge()` unions the two sides of an `if`, so that a single
/// branch marks a name as bound.
#[derive(Clone, Default)]
struct FlowState {
    bound: FxHashSet<String>,
}

impl FlowState {
    /// Save the current state, to rewind to or to seed a child scan unit from.
    fn snapshot(&self) -> FlowState {
        self.clone()
    }

    /// Rewind to `snapshot`, dropping any bindings recorded since it was taken.
    fn restore(&mut self, snapshot: FlowState) {
        *self = snapshot;
    }

    /// Union `snapshot` in, so a name reads as bound here if it was bound on
    /// either path. This is the `if`/`else` join.
    fn merge(&mut self, snapshot: FlowState) {
        self.bound.extend(snapshot.bound);
    }

    /// Record `name` as bound from here on.
    fn bind(&mut self, name: String) {
        self.bound.insert(name);
    }

    /// Whether `name` is bound at the current point.
    fn is_bound(&self, name: &str) -> bool {
        self.bound.contains(name)
    }

    /// Drop all bindings, to start a fresh scan unit (see `begin_scan()`).
    fn clear(&mut self) {
        self.bound.clear();
    }
}

/// Tracks eager `Nested` NSE bodies scanned inline during the scan.
///
/// An eager `Nested` body like `local()` runs immediately at its call site, so
/// we scan it inline instead of deferring it to the walk. `open` is the stack
/// of bodies being scanned right now, with the innermost on top.
/// `record_owner_name()` routes a binding to the top so names land on the body
/// that owns them. When a descent finishes, its names move to `pending`, keyed
/// by the body's range.
///
/// `pending` is keyed by range rather than written straight into
/// `bound_names[scope]` because the body's arena scope doesn't exist yet: the
/// walk allocates scopes in preorder, and allocating one mid-scan would break
/// the `Scope::descendants` invariant. The range is the body's pre-arena
/// identity until the walk pushes its scope.
///
/// Once the walk pushes that scope, it installs the pending names into it
/// instead of re-scanning. It does this before collecting the body, because a
/// lazy child inside (a function or lazy NSE body) runs later than the walk
/// reaches it, so it can reference a binding defined further down this scope.
/// Resolving that name checks whether an enclosing scope binds it
/// (`scope_binds_anywhere()`), and the walk hasn't recorded that binding yet, so
/// the scan-populated bound set has to be complete up front. That's the reason
/// the scan collects bound names ahead of the walk at all.
#[derive(Default)]
struct EagerNestedDescent {
    open: Vec<BoundNames>,
    pending: FxHashMap<TextRange, BoundNames>,
}

/// All definitions in a scope, collected by the scan pass before the
/// walk. Skips child-scope bodies (nested functions and `Nested` NSE bodies).
struct BoundNames {
    by_name: FxHashSet<String>,
}

impl BoundNames {
    fn new() -> Self {
        Self {
            by_name: FxHashSet::default(),
        }
    }

    fn add(&mut self, name: String) {
        self.by_name.insert(name);
    }

    fn binds(&self, name: &str) -> bool {
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
