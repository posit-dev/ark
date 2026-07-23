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
use aether_syntax::AnyRValue;
use aether_syntax::RBinaryExpression;
use aether_syntax::RRoot;
use aether_syntax::RSyntaxKind;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use oak_index_vec::Idx;
use oak_index_vec::IndexVec;
use rustc_hash::FxHashMap;
use scan::BoundNames;
use scan::CallResolution;
use scan::EagerNestedDescent;
use scan::FlowState;

use crate::resolver::ImportsResolver;
use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionId;
use crate::semantic_index::EnclosingSnapshotId;
use crate::semantic_index::EnclosingSnapshotKey;
use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;
use crate::semantic_index::NamespaceAccess;
use crate::semantic_index::Scope;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticCall;
use crate::semantic_index::SemanticDiagnostic;
use crate::semantic_index::SemanticIndex;
use crate::semantic_index::SymbolFlags;
use crate::semantic_index::SymbolId;
use crate::semantic_index::SymbolTableBuilder;
use crate::semantic_index::Use;
use crate::semantic_index::UseId;
use crate::use_def_map::UseDefMapBuilder;

mod builder_nse;
mod scan;
mod walk;

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
    current_scope: ScopeId,
    // Diagnostics collected during the build and logged on `finish()`. A minimal
    // channel for now, no user-facing surface.
    diagnostics: Vec<SemanticDiagnostic>,
    scan: ScanState,
    walk: WalkState,
}

/// State owned by the scan pass: its working state plus the products the walk
/// reads back (`bound_names`, `call_resolutions`, `eager_descent.pending`).
/// The walk also writes `bound_names`, but only to install scan-produced data:
/// the lockstep push in `push_scope()` and the pending install in
/// `collect_nse_argument()`.
struct ScanState {
    bound_names: IndexVec<ScopeId, BoundNames>,
    // Per-call facts resolved by the scanner in flow order, keyed by the call's
    // range. See `CallResolution`.
    call_resolutions: FxHashMap<TextRange, CallResolution>,
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

/// State written by the walk pass: the per-scope arenas and the flat outputs
/// carried into the final [`SemanticIndex`]. Note that the scan reads some of
/// this data mid-flight, which is why we keep both states in a single builder.
struct WalkState {
    symbol_tables: IndexVec<ScopeId, SymbolTableBuilder>,
    definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
    uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
    use_def_maps: IndexVec<ScopeId, UseDefMapBuilder>,
    enclosing_snapshots: FxHashMap<EnclosingSnapshotKey, (ScopeId, EnclosingSnapshotId)>,
    // Snapshots shared across every use of a free variable in lazy contexts,
    // keyed by (nested scope, nested symbol).
    lazy_snapshots: FxHashMap<(ScopeId, SymbolId), (ScopeId, EnclosingSnapshotId)>,
    semantic_calls: Vec<SemanticCall>,
    namespace_accesses: Vec<NamespaceAccess>,
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
            current_scope: file_scope,
            diagnostics: Vec::new(),
            resolver,
            scan: ScanState {
                bound_names,
                call_resolutions: FxHashMap::default(),
                flow_state: FlowState::default(),
                enclosing_flow: FxHashMap::default(),
                attached_flow: Vec::new(),
                eager_descent: EagerNestedDescent::default(),
            },
            walk: WalkState {
                symbol_tables,
                definitions,
                uses,
                use_def_maps,
                enclosing_snapshots: FxHashMap::default(),
                lazy_snapshots: FxHashMap::default(),
                semantic_calls: Vec::new(),
                namespace_accesses: Vec::new(),
            },
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

        self.walk.symbol_tables.push(SymbolTableBuilder::new());
        self.walk.definitions.push(IndexVec::new());
        self.walk.uses.push(IndexVec::new());
        self.walk.use_def_maps.push(UseDefMapBuilder::new());
        self.scan.bound_names.push(BoundNames::new());

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

    /// The scope that owns definitions of a `Current + Lazy` NSE scope. The
    /// climb is iterative to handle e.g. `on_load(on_load(...))`. Every other
    /// scope kind (`File`, `Function`, `Nse(Nested, _)`) owns its definitions
    /// and stops the climb.
    fn definition_owner(&self) -> Option<ScopeId> {
        let mut scope = self.scopes[self.current_scope].parent?;
        while matches!(
            self.scopes[scope].kind,
            ScopeKind::Nse(EvalEnv::Current, EvalTiming::Lazy)
        ) {
            scope = self.scopes[scope].parent?;
        }
        Some(scope)
    }

    /// Whether `scope` binds `name` anywhere, regardless of flow position: an
    /// already-recorded `IS_BOUND` definition or a pre-scanned assignment. The
    /// pre-scan covers definitions the walk hasn't reached yet in this scope.
    fn scope_binds_anywhere(&self, scope: ScopeId, name: &str) -> bool {
        self.walked_binding(scope, name).is_some() || self.scan.bound_names[scope].binds(name)
    }

    /// The site where `scope` binds `name`, matching what
    /// [`scope_binds_anywhere`](Self::scope_binds_anywhere) counts as a binding
    /// (so it returns `Some` on exactly the same names). Prefers the
    /// scan-collected site in `bound_names`, falling back to the range of an
    /// already-walked `IS_BOUND` definition (e.g. a parameter, which the scan
    /// seeds straight into `flow_state` without a `bound_names` entry). Used to
    /// point the lazy-shadow diagnostic at the overwrite.
    fn scope_binding_range(&self, scope: ScopeId, name: &str) -> Option<TextRange> {
        if let Some(range) = self.scan.bound_names[scope].binding_range(name) {
            return Some(range);
        }

        // `IS_BOUND` always has a matching `Definition` row (see the invariant
        // in `resolve_symbol()`), so the find never misses when the flag is set.
        let sym_id = self.walked_binding(scope, name)?;
        self.walk.definitions[scope]
            .iter()
            .find(|(_id, def)| def.symbol == sym_id)
            .map(|(_id, def)| def.range)
    }

    /// The symbol `name` interns to in `scope`, if the walk has already recorded
    /// an `IS_BOUND` definition for it.
    fn walked_binding(&self, scope: ScopeId, name: &str) -> Option<SymbolId> {
        let sym_id = self.walk.symbol_tables[scope].id(name)?;
        self.walk.symbol_tables[scope]
            .symbol(sym_id)
            .flags()
            .contains(SymbolFlags::IS_BOUND)
            .then_some(sym_id)
    }

    fn finish(mut self) -> SemanticIndex {
        self.scopes[ScopeId::from(0)].descendants.end = self.scopes.next_id();

        // TODO(diagnostics): Diagnostics are not surfaced yet, so log them for now
        for diagnostic in &self.diagnostics {
            match diagnostic {
                SemanticDiagnostic::LazyShadowAmbiguity {
                    name,
                    call_range,
                    overwrite_range,
                } => log::warn!(
                    "Lazy-shadow ambiguity: callee `{name}` at {call_range:?} is recognized \
                     as effectful, but a lazy-crossed ancestor binds it at {overwrite_range:?} \
                     with undetermined timing"
                ),
            }
        }

        let symbol_tables = self
            .walk
            .symbol_tables
            .into_iter()
            .map(|b| Arc::new(b.build()))
            .collect();

        // The file scope's exit flow state is the file's exports. Capture it
        // before the builders are consumed below.
        let file_final_bindings = self.walk.use_def_maps[ScopeId::from(0)]
            .final_bindings()
            .clone();

        let use_def_maps: IndexVec<ScopeId, _> = self
            .walk
            .use_def_maps
            .into_iter()
            .zip(self.walk.uses.iter())
            .map(|(b, (_, uses))| Arc::new(b.finish(uses)))
            .collect();

        SemanticIndex::new(
            self.scopes,
            symbol_tables,
            self.walk.definitions,
            self.walk.uses,
            use_def_maps,
            self.walk.enclosing_snapshots,
            self.walk.semantic_calls,
            self.walk.namespace_accesses,
            self.diagnostics,
            file_final_bindings,
        )
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
