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
//! eager bindings and is allowed to stay coarse: at an `if` join it only keeps
//! the names consistently bound on every path. The walk builds the precise
//! structures, such as the use-def map, where conditionality is recorded as
//! `may_be_unbound`.

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
use scan::BindingSites;
use scan::BodyScan;
use scan::CallResolution;
use scan::DeferredBody;
use scan::FlowState;
use scan::OpenScope;

use crate::resolver::ImportsResolver;
use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionId;
use crate::semantic_index::EnclosingSnapshotId;
use crate::semantic_index::EnclosingSnapshotKey;
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

mod effects;
mod scan;
mod walk;

/// Build a [`SemanticIndex`] from a parsed R file with cross-file
/// information supplied by `resolver`. See [`ImportsResolver`] for the
/// available impls.
///
/// See the module docs for the scan/walk split. The scan
/// ([`scan_expression`]) runs first over each scope, then the walk
/// ([`walk_expression`]) reuses its decisions and pushes NSE scopes inline.
///
/// [`scan_expression`]: SemanticIndexBuilder::scan_expression
/// [`walk_expression`]: SemanticIndexBuilder::walk_expression
pub fn build_index(root: &RRoot, resolver: impl ImportsResolver) -> SemanticIndex {
    let range = root.syntax().text_trimmed_range();

    let mut builder = SemanticIndexBuilder::new(range, resolver);
    builder.begin_scan();
    builder.scan_expression_list(&root.expressions());
    builder.scan_deferred_bodies(0);
    builder.walk_expression_list(&root.expressions());
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

/// State owned by the scan pass.
///
/// Binding state comes in two views, because eager and lazy code ask
/// different questions.
///
/// - An eager callee is shadowed only by bindings that already ran.
///   `bound_so_far` reflects this view. It rewinds at branch joins and is
///   reseeded for each scan unit. Forward bindings (defined later) and
///   deferred bindings (`on.exit()`, `<<-`) don't enter `bound_so_far`.
/// - A lazy body runs after its scope has finished and resolves symbols
///   in the whole scope. `bound_anywhere` reflects this view.
///
/// Both views are written together by `record_binding()`. They diverge on
/// two rules. Names inherited from enclosing scopes seed `bound_so_far` only,
/// via `begin_scan()`. Names bound by a deferred body reach the owner's
/// `bound_anywhere` only, because the deferred scan restores `bound_so_far`
/// afterwards, so the name is visible to lazy readers without shadowing an
/// eager callee after the call.
struct ScanState {
    bound_anywhere: IndexVec<ScopeId, BindingSites>,
    bound_so_far: FlowState,
    // Scopes the scan has entered that are not yet allocated in the arena,
    // innermost last.
    open_scopes: Vec<OpenScope>,
    // What the scan prepared for each child body, keyed by the body's range.
    // See [`BodyScan`].
    body_scans: FxHashMap<TextRange, BodyScan>,
    // Packages attached on every path so far, the attach analog of
    // `bound_so_far` (file level and eager NSE descents, appended only when
    // `!is_lazy()`). A `library()` on only one branch, or in a loop body that
    // may not run, drops at the join. An eager callee reads the flow-precise
    // prefix during the file scan. A lazy callee reads the end-of-file value
    // during the walk, paired with `attached_inherited` for what was live where
    // that body was defined.
    attached_so_far: Vec<String>,
    // Attaches that were live where the current scan unit was defined, cleared
    // and reseeded by `begin_scan()`. A lazy body inherits them, e.g. in
    // `if (cond) { library(shiny); f <- function() reactive({ ... }) }`. Empty
    // for the file scope and for any unit defined outside a branch.
    attached_inherited: Vec<String>,
    // Per-call facts resolved by the scanner in flow order, keyed by the call's
    // range. See `CallResolution`.
    call_resolutions: FxHashMap<TextRange, CallResolution>,
    // `Current + Lazy` bodies (e.g. `rlang::on_load()`) queued at their call
    // sites, scanned when their enclosing scan unit finishes.
    deferred_bodies: Vec<DeferredBody>,
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
        let mut bound_anywhere = IndexVec::new();

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
        bound_anywhere.push(BindingSites::new());

        Self {
            scopes,
            current_scope: file_scope,
            diagnostics: Vec::new(),
            resolver,
            scan: ScanState {
                bound_anywhere,
                call_resolutions: FxHashMap::default(),
                bound_so_far: FlowState::default(),
                body_scans: FxHashMap::default(),
                attached_so_far: Vec::new(),
                attached_inherited: Vec::new(),
                open_scopes: Vec::new(),
                deferred_bodies: Vec::new(),
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
        self.scan.bound_anywhere.push(BindingSites::new());

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
    fn enclosing_owner(&self) -> Option<ScopeId> {
        let mut scope = self.scopes[self.current_scope].parent?;
        while !self.scopes[scope].kind.owns_bindings() {
            scope = self.scopes[scope].parent?;
        }
        Some(scope)
    }

    /// Whether `scope` binds `name` anywhere, regardless of flow position: an
    /// already-recorded `IS_BOUND` definition or a pre-scanned assignment. The
    /// pre-scan covers definitions the walk hasn't reached yet in this scope.
    fn scope_binds_anywhere(&self, scope: ScopeId, name: &str) -> bool {
        self.walked_binding(scope, name).is_some() || self.scan.bound_anywhere[scope].binds(name)
    }

    /// The site where `scope` binds `name`, matching what
    /// [`scope_binds_anywhere`](Self::scope_binds_anywhere) counts as a binding
    /// (so it returns `Some` on exactly the same names). Prefers the
    /// scan-collected site in `bound_anywhere`, falling back to the range of an
    /// already-walked `IS_BOUND` definition (e.g. a parameter, which the scan
    /// seeds straight into `bound_so_far` without a `bound_anywhere` entry). Used to
    /// point the lazy-shadow diagnostic at the overwrite.
    fn scope_binding_range(&self, scope: ScopeId, name: &str) -> Option<TextRange> {
        if let Some(range) = self.scan.bound_anywhere[scope].binding_range(name) {
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
