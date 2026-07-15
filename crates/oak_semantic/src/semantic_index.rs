use std::ops::Range;
use std::sync::Arc;

use aether_syntax::AnyRExpression;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use aether_syntax::RForStatement;
use aether_syntax::RParameter;
use biome_rowan::AstPtr;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_core::range::Ranged;
use oak_index_vec::define_index;
use oak_index_vec::IndexVec;
use rustc_hash::FxHashMap;
use url::Url;

use crate::use_def_map::Bindings;
use crate::use_def_map::UseDefMap;

// File-local scope identifier
define_index!(ScopeId);

// Scope-local symbol identifier
define_index!(SymbolId);

// Scope-local definition site identifier
define_index!(DefinitionId);

// Scope-local use site identifier
define_index!(UseId);

// Scope-local enclosing snapshot identifier
define_index!(EnclosingSnapshotId);

// File-local declaration identifier, indexing the builder's arena of
// `Declaration`s carried by local bindings
define_index!(DeclId);

// One `SemanticIndex` per R source file. This reflects the physical reality of
// a single file. Cross-file resolution (e.g. package namespaces, sourced
// scripts) is a separate concern handled by layers above this.
// Consequently, `ScopeId(0)` is always the top-level scope of a file.
//
// Scopes, symbol tables, definitions, and uses are stored in parallel arrays
// (all indexed by `ScopeId`) rather than bundled into a single struct, so
// that each can be cached and invalidated independently (when salsa is
// introduced).
#[derive(Debug)]
#[cfg_attr(feature = "salsa", derive(salsa::Update))]
#[cfg_attr(feature = "testing", derive(PartialEq, Eq))]
pub struct SemanticIndex {
    scopes: IndexVec<ScopeId, Scope>,

    // Heavy per-scope tables are `Arc`-wrapped so narrow tracked queries
    // (e.g. `symbol_table(db, file, scope) -> Arc<SymbolTable>`) can
    // return them cheaply, and salsa's storage uses `Arc::ptr_eq` as a
    // fast path during update comparisons. Matches ty's pattern.
    symbol_tables: IndexVec<ScopeId, Arc<SymbolTable>>,

    // Flat per-scope lists of definition and use sites. These support rename
    // and go-to-definition by letting us find all sites for a given symbol
    // without control-flow analysis.
    //
    // In ty, definitions are salsa-tracked `Definition<'db>` structs (stored
    // in `definitions_by_node`), and use sites are tracked via `AstIds`
    // (a per-scope map from AST node positions to `ScopedUseId`). When we
    // introduce salsa, these lists may be restructured to match.
    //
    // Use-def maps layer on top of these lists. A use-def map tracks which
    // definitions reach each use through control flow, referencing
    // `DefinitionId` and `UseId` indices into these arenas.
    definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
    uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,

    // Per-scope flow-sensitive map from each use site to the set of
    // definitions that can reach it. Built alongside the other arrays
    // during the tree walk. `Arc`-wrapped for fast Salsa comparisons.
    use_def_maps: IndexVec<ScopeId, Arc<UseDefMap>>,

    // For each free variable in a nested scope, maps to the enclosing scope and
    // snapshot where that symbol is bound.
    enclosing_snapshots: FxHashMap<EnclosingSnapshotKey, (ScopeId, EnclosingSnapshotId)>,

    // Cross-file call sites recorded during indexing, such as `library()`
    // attachments or `source()` injections.
    semantic_calls: Vec<SemanticCall>,

    // Namespace accesses recorded during indexing, i.e. `package::symbol` or
    // `package:::symbol`
    namespace_accesses: Vec<NamespaceAccess>,

    // Diagnostics surfaced during indexing, for downstream consumers to turn
    // into user-facing diagnostics.
    diagnostics: Vec<SemanticDiagnostic>,

    // The file scope's exit flow state: for each top-level symbol, the
    // definitions still in effect once the file has run top to bottom. This is
    // the file's exports (see `exports()`). Only the file scope's exit state is
    // ever needed, so we keep this one copy rather than per-scope state.
    final_bindings: IndexVec<SymbolId, Bindings>,
}

impl SemanticIndex {
    pub(crate) fn new(
        scopes: IndexVec<ScopeId, Scope>,
        symbol_tables: IndexVec<ScopeId, Arc<SymbolTable>>,
        definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
        uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
        use_def_maps: IndexVec<ScopeId, Arc<UseDefMap>>,
        enclosing_snapshots: FxHashMap<EnclosingSnapshotKey, (ScopeId, EnclosingSnapshotId)>,
        semantic_calls: Vec<SemanticCall>,
        namespace_accesses: Vec<NamespaceAccess>,
        diagnostics: Vec<SemanticDiagnostic>,
        final_bindings: IndexVec<SymbolId, Bindings>,
    ) -> Self {
        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
            use_def_maps,
            enclosing_snapshots,
            semantic_calls,
            namespace_accesses,
            diagnostics,
            final_bindings,
        }
    }

    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id]
    }

    pub fn symbols(&self, scope: ScopeId) -> &Arc<SymbolTable> {
        &self.symbol_tables[scope]
    }

    pub fn definitions(&self, scope: ScopeId) -> &IndexVec<DefinitionId, Definition> {
        &self.definitions[scope]
    }

    pub fn uses(&self, scope: ScopeId) -> &IndexVec<UseId, Use> {
        &self.uses[scope]
    }

    pub fn use_def_map(&self, scope: ScopeId) -> &Arc<UseDefMap> {
        &self.use_def_maps[scope]
    }

    /// Top-level definitions exported by this file (definitions in the file scope).
    /// Includes `Import`-kind forwarding definitions from `source()` calls.
    ///
    /// For each name, the definitions still in effect once the file has run top
    /// to bottom, which is what another file sees after `source()`-ing this one.
    /// A name rebound in sequence keeps only the last def (the earlier one was
    /// overwritten), so `x <- 1; x <- 2` exports just the second. A name bound
    /// on both arms of an `if`/`else` (`if (cond) x <- 1 else x <- 2`) keeps
    /// both, since either could be the one that ran. Definitions come back in
    /// definition order.
    pub fn exports(&self) -> FxHashMap<&str, Vec<(DefinitionId, &Definition)>> {
        let file_scope = ScopeId::from(0);
        let symbols = &self.symbol_tables[file_scope];
        let defs = &self.definitions[file_scope];

        let mut exports: FxHashMap<&str, Vec<(DefinitionId, &Definition)>> = FxHashMap::default();
        for (symbol_id, bindings) in self.final_bindings.iter() {
            if bindings.definitions().is_empty() {
                continue;
            }
            let name = symbols.symbol(symbol_id).name();
            let list = exports.entry(name).or_default();
            for &def_id in bindings.definitions() {
                list.push((def_id, &defs[def_id]));
            }
        }

        exports
    }

    /// Package names from `library()` / `require()` calls that run at the
    /// file's own top level.
    ///
    /// A `library()` reached only by running a lazy body attaches only if that
    /// body runs, which may be never, so it does not count here. That covers a
    /// function body, a lazy NSE argument, and an eager `local()` *nested*
    /// inside one (see [`Self::scope_is_eager`]). This is the load-time
    /// search-path view: what another file sees after `source()`-ing this one,
    /// and what an eager callee resolves against. For every `library()`
    /// regardless of context, see [`Self::attached_packages_anywhere`].
    pub fn attached_packages(&self) -> Vec<&str> {
        self.semantic_calls
            .iter()
            .filter(|call| self.scope_is_eager(call.scope))
            .filter_map(|call| match &call.kind {
                SemanticCallKind::Attach { package } => Some(package.as_str()),
                SemanticCallKind::Source { .. } => None,
            })
            .collect()
    }

    /// Every `library()` / `require()` call in the file, including those in
    /// function bodies and other lazy contexts.
    ///
    /// Over-approximates the load-time search path, since a lazy body may never
    /// run, so most callers want [`Self::attached_packages`]. Use this only
    /// where a lazy attach still counts, e.g. "which packages does this file
    /// depend on" for workspace dependency discovery.
    pub fn attached_packages_anywhere(&self) -> Vec<&str> {
        self.semantic_calls
            .iter()
            .filter_map(|call| match &call.kind {
                SemanticCallKind::Attach { package } => Some(package.as_str()),
                SemanticCallKind::Source { .. } => None,
            })
            .collect()
    }

    /// Whether `scope` runs during the file's own top-level execution, i.e. no
    /// enclosing scope is lazy.
    fn scope_is_eager(&self, scope_id: ScopeId) -> bool {
        let mut ancestor_id = Some(scope_id);

        while let Some(id) = ancestor_id {
            let ancestor_scope = self.scope(id);
            if ancestor_scope.kind.is_lazy() {
                return false;
            }
            ancestor_id = ancestor_scope.parent;
        }

        true
    }

    /// Cross-file call sites (`library()`, `source()`, …) recorded
    /// during indexing.
    pub fn semantic_calls(&self) -> &[SemanticCall] {
        &self.semantic_calls
    }

    /// Namespace accesses recorded during indexing, i.e. `package::symbol` or
    /// `package:::symbol`
    pub fn namespace_accesses(&self) -> &[NamespaceAccess] {
        &self.namespace_accesses
    }

    /// Diagnostics surfaced during indexing, for downstream consumers to turn
    /// into user-facing diagnostics.
    pub fn diagnostics(&self) -> &[SemanticDiagnostic] {
        &self.diagnostics
    }

    /// Find the innermost scope containing `offset`.
    pub fn scope_at(&self, offset: biome_rowan::TextSize) -> (ScopeId, &Scope) {
        // Start at the file scope
        let mut current = ScopeId::from(0);
        'outer: loop {
            for child_id in self.child_scope_ids(current) {
                if self.scopes[child_id].range.contains(offset) {
                    current = child_id;
                    continue 'outer;
                }
            }
            return (current, &self.scopes[current]);
        }
    }

    /// Find the definition site at `offset`, if any.
    pub fn definition_at(&self, offset: TextSize) -> Option<(ScopeId, DefinitionId, &Definition)> {
        let (scope, _) = self.scope_at(offset);

        // Definitions with empty ranges (e.g. imports) are naturally excluded
        // here since they can't contain the offset
        let (id, def) = self.definitions(scope).contains(offset)?;
        Some((scope, id, def))
    }

    /// Find the use site at `offset`, if any.
    pub fn use_at(&self, offset: TextSize) -> Option<(ScopeId, UseId, &Use)> {
        let (scope, _) = self.scope_at(offset);
        let (id, use_site) = self.uses(scope).contains(offset)?;
        Some((scope, id, use_site))
    }

    /// All use sites for `name`, across every scope in the file. The
    /// many-sites counterpart to [`use_at`](Self::use_at), with the same
    /// `(ScopeId, UseId, &Use)` element.
    pub fn uses_of(&self, name: &str) -> Vec<(ScopeId, UseId, &Use)> {
        let mut uses = Vec::new();
        for scope_id in self.scope_ids() {
            let Some(symbol_id) = self.symbols(scope_id).id(name) else {
                continue;
            };
            for (use_id, use_site) in self.uses(scope_id).iter() {
                if use_site.symbol() == symbol_id {
                    uses.push((scope_id, use_id, use_site));
                }
            }
        }
        uses
    }

    /// Iterate direct child scopes of `scope`.
    pub fn child_scope_ids(&self, scope_id: ScopeId) -> ChildScopeIdsIter<'_> {
        let descendants = &self.scopes[scope_id].descendants;
        ChildScopeIdsIter {
            index: self,
            current: descendants.start,
            end: descendants.end,
        }
    }

    /// Iterate over every scope in the file (source order, file scope first).
    pub fn scope_ids(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.scopes.iter().map(|(id, _)| id)
    }

    /// Walk from `scope` up through ancestors to the file root. Note that
    /// `scope` itself is included in the ancestors.
    pub fn ancestor_scope_ids(&self, scope_id: ScopeId) -> AncestorScopeIdsIter<'_> {
        AncestorScopeIdsIter {
            index: self,
            current: Some(scope_id),
        }
    }

    /// Resolve a name starting from `scope`, walking up the scope chain.
    /// Returns the scope that owns the binding, the `DefinitionId` of the
    /// first matching [`Definition`] in that scope (source-order first),
    /// and a borrow of the definition itself.
    pub fn resolve(
        &self,
        name: &str,
        scope: ScopeId,
    ) -> Option<(ScopeId, DefinitionId, &Definition)> {
        for ancestor in self.ancestor_scope_ids(scope) {
            let Some(symbol_id) = self.symbol_tables[ancestor].id(name) else {
                continue;
            };
            if !self.symbol_tables[ancestor]
                .symbol(symbol_id)
                .flags()
                .contains(SymbolFlags::IS_BOUND)
            {
                continue;
            }

            // `IS_BOUND` iff at least one `Definition` was recorded for
            // this symbol. The builder maintains this in lockstep, so the
            // panic below is unreachable.
            let (def_id, def) = match self.definitions[ancestor]
                .iter()
                .find(|(_id, d)| d.symbol() == symbol_id)
            {
                Some(pair) => pair,
                None => unreachable!(
                    "IS_BOUND symbol {name:?} in scope {ancestor:?} has no \
                    Definition: oak_semantic builder invariant violated"
                ),
            };
            return Some((ancestor, def_id, def));
        }

        None
    }

    /// All definitions that could reach the use at `(scope, use_id)`.
    ///
    /// The local use-def bindings always count. The enclosing-scope snapshot
    /// also counts when `may_be_unbound` is true. That happens when the local
    /// binding doesn't cover every control-flow path, so execution can fall
    /// through to the outer scope.
    ///
    /// When `may_be_unbound` is false we deliberately skip the enclosing scope.
    /// Otherwise a shadowed inner use would also bind to the outer def of the
    /// same name.
    pub fn reaching_definitions(
        &self,
        scope_id: ScopeId,
        use_id: UseId,
    ) -> impl Iterator<Item = (ScopeId, DefinitionId)> + '_ {
        let bindings = self.use_def_map(scope_id).bindings_at_use(use_id);
        let local = bindings.definitions().iter().map(move |&d| (scope_id, d));

        let enclosing = if bindings.may_be_unbound() {
            let symbol_id = self.uses(scope_id)[use_id].symbol();
            self.enclosing_bindings(scope_id, symbol_id)
        } else {
            None
        };
        let enclosing_iter = enclosing.into_iter().flat_map(|(scope, bindings)| {
            bindings.definitions().iter().map(move |&def| (scope, def))
        });

        local.chain(enclosing_iter)
    }

    /// Resolve a free variable's bindings from the enclosing scope.
    ///
    /// When a use in `scope` may be unbound (`may_be_unbound: true`), some
    /// control-flow paths fall through to an enclosing scope. This looks up
    /// the enclosing snapshot that was registered during the build and
    /// returns the ancestor scope and its bindings. This covers both purely
    /// free variables (no local definitions) and conditionally defined
    /// variables (local definitions exist but don't cover all paths).
    ///
    /// Returns `None` if no enclosing snapshot was registered (e.g. the
    /// variable is truly global or from the search path) and needs
    /// cross-file resolution.
    pub fn enclosing_bindings(
        &self,
        scope: ScopeId,
        symbol: SymbolId,
    ) -> Option<(ScopeId, &Bindings)> {
        let key = EnclosingSnapshotKey {
            nested_scope: scope,
            nested_symbol: symbol,
        };
        let &(enclosing_scope, snapshot_id) = self.enclosing_snapshots.get(&key)?;
        let bindings = self.use_def_maps[enclosing_scope].enclosing_snapshot(snapshot_id);
        Some((enclosing_scope, bindings))
    }
}

/// Key for looking up an enclosing snapshot. Keyed by the nested scope and the
/// symbol's `SymbolId` in the nested scope's symbol table (not the enclosing
/// scope's), so consumers can do an O(1) lookup directly from a `UseId` without
/// re-walking the ancestor chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnclosingSnapshotKey {
    pub nested_scope: ScopeId,
    pub nested_symbol: SymbolId,
}

// --- Scope ---

// A lexical scope within a file. Each scope has its own symbol table,
// definitions, and uses. The scope chain (via `parent`) is always bounded
// by the file: walking `parent` from any scope eventually reaches the
// `File` scope which itself has `parent: None`.
//
// `function()` creates `Function` scopes. NSE constructs like `local()`,
// `with()`, `test_that()` create `Nse` scopes, recognized by resolving the
// call target against for their effects annotations during the walk.
#[derive(Debug, PartialEq, Eq)]
pub struct Scope {
    pub(crate) parent: Option<ScopeId>,
    pub(crate) kind: ScopeKind,
    pub(crate) range: TextRange,
    // Scopes are allocated in preorder, so a scope's descendants always
    // occupy a contiguous range of IDs. This lets `child_scopes` and
    // `scope_at` work by range arithmetic instead of pointer chasing.
    pub(crate) descendants: Range<ScopeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    // The file's top-level scope. Every file has exactly one, always at
    // `ScopeId(0)`. Unresolved names fall through to this scope, where
    // cross-file resolution (package namespace, session, etc.) takes over.
    File,
    Function,
    Nse(NseScope, NseTiming),
}

/// Where definitions in an NSE scope land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NseScope {
    /// Definitions go to the current (parent) environment.
    /// e.g. `rlang::on_load()`
    Current,
    /// Definitions go to a nested environment.
    /// e.g. `local()`, `test_that()`, `with()`
    Nested,
}

/// Whether an NSE scope evaluates eagerly (at the call site) or lazily
/// (at an unknown later time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NseTiming {
    Eager,
    Lazy,
}

impl ScopeKind {
    /// Whether free variables in this scope resolve against the union of all
    /// enclosing definitions (lazy) or against a point-in-time snapshot at the
    /// call site (eager). `Function` bodies run at an unknown later time, so
    /// they're always lazy.
    pub fn is_lazy(self) -> bool {
        match self {
            ScopeKind::File => false,
            ScopeKind::Function => true,
            ScopeKind::Nse(_, laziness) => laziness == NseTiming::Lazy,
        }
    }
}

impl Scope {
    pub fn parent(&self) -> Option<ScopeId> {
        self.parent
    }

    pub fn kind(&self) -> ScopeKind {
        self.kind
    }

    pub fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for Scope {
    fn range(&self) -> TextRange {
        self.range()
    }
}

// --- Symbol table (per scope) ---

// Read-only after construction. The builder uses `SymbolTableBuilder`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SymbolTable {
    symbols: IndexVec<SymbolId, Symbol>,

    // Note that ty uses `hashbrown::HashTable` to provide lookup by name
    // without storing the name twice
    by_name: FxHashMap<String, SymbolId>,
}

impl SymbolTable {
    pub fn get(&self, name: &str) -> Option<&Symbol> {
        self.by_name.get(name).map(|&id| &self.symbols[id])
    }

    pub fn id(&self, name: &str) -> Option<SymbolId> {
        self.by_name.get(name).copied()
    }

    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id]
    }

    pub fn iter(&self) -> impl Iterator<Item = (SymbolId, &Symbol)> {
        self.symbols.iter()
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }
}

#[derive(Debug, Default)]
pub(crate) struct SymbolTableBuilder {
    table: SymbolTable,
}

impl SymbolTableBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn intern(&mut self, name: &str, flags: SymbolFlags) -> SymbolId {
        if let Some(&id) = self.table.by_name.get(name) {
            self.table.symbols[id].flags.insert(flags);
            return id;
        }
        let id = self.table.symbols.push(Symbol {
            name: name.to_owned(),
            flags,
        });
        self.table.by_name.insert(name.to_owned(), id);
        id
    }

    pub(crate) fn build(self) -> SymbolTable {
        self.table
    }
}

impl std::ops::Deref for SymbolTableBuilder {
    type Target = SymbolTable;

    fn deref(&self) -> &SymbolTable {
        &self.table
    }
}

// --- Symbol ---

// The unique identity of a name within a scope. A symbol is created the first
// time a name is encountered (as a binding or a use), and subsequent
// occurrences of the same name in that scope merge their flags into the
// existing symbol.
//
// Definitions and uses reference symbols via `SymbolId`. The symbol itself
// doesn't track where it's defined or used, that's the job of the `Definition`
// and `Use` lists. The symbol just records the name and summary flags (bound?
// used? parameter?).
//
// In ty, symbols live in a `PlaceTable` (which generalizes to include member
// access like `x.y`). `resolve_symbol()` walks the scope chain looking for a
// symbol with `IS_BOUND`. Future type inference will look up the symbol's
// definitions and infer a type from them.
#[derive(Debug, PartialEq, Eq)]
pub struct Symbol {
    pub(crate) name: String,
    pub(crate) flags: SymbolFlags,
}

impl Symbol {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn flags(&self) -> SymbolFlags {
        self.flags
    }
}

// Summary bits accumulated during the tree walk, so queries like "is this
// symbol bound in this scope?" are O(1) without scanning binding/use lists.
// Bitflags rather than a struct of bools for compact storage and composability.
//
// These flags are scope-level summaries, not fine-grained enough to
// implement LSP features directly. For example, `IS_BOUND` says "x is
// bound somewhere in this scope" but can't answer "which definition of x
// reaches this point?" or "is x defined before this use?". Use-def maps
// provide that precision. The flags remain useful for scope-level queries
// like `resolve_symbol` (which walks the scope chain checking
// `IS_BOUND`). They can also be useful as filters for
// short-circuiting unneeded expensive operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolFlags(u8);

impl SymbolFlags {
    // Referenced by name in this scope.
    pub const IS_USED: Self = Self(1 << 0);
    // Given a value: assignment (`<-`, `=`, `->`) or parameter definition.
    pub const IS_BOUND: Self = Self(1 << 1);
    // Appears in a function's formal parameter list.
    pub const IS_PARAMETER: Self = Self(1 << 2);
    // Target of a super-assignment (`<<-`, `->>`). Recorded in the scope
    // where the expression lexically appears, not in the target ancestor
    // scope. Not visible to `resolve_symbol` (which checks `IS_BOUND`).
    pub const IS_SUPER_BOUND: Self = Self(1 << 3);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns `true` if `self` and `other` share at least one flag.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

// --- Definition and Use sites ---

// A site where a symbol is bound (given a value) or declared (given a type
// constraint). Bridges the scope/symbol layer (which symbol, which scope)
// and the syntax layer (what construct created it, via `DefinitionKind`).
//
// A symbol can have multiple definitions in the same scope (e.g. `x <- 1`
// then `x <- 2`). In ty, `DefinitionKind` classifies the construct
// (assignment, parameter, for-variable) and carries a reference to the
// syntax node. This split matters for salsa: the kind is marked `#[no_eq]`
// so that editing the RHS of an assignment re-runs type inference for that
// definition without invalidating the definition's identity (file + scope +
// place) or the UseDefMap.
//
// Type inference will eventually take a definition as input and inspect
// the syntax node (via the kind) to determine the type.
//
// ty also classifies definitions into categories: `Binding` (gives a value),
// `Declaration` (constrains a type), or `DeclarationAndBinding` (both).
// For R, most definitions are bindings today, but function definitions are
// implicitly declarations (they declare a name as a function with a specific
// signature). Future `declare()` annotations will also produce pure
// declarations.
#[derive(Debug, PartialEq, Eq)]
pub struct Definition {
    pub(crate) kind: DefinitionKind,
    // TODO(salsa): Should become a PlaceId (like ty's `ScopedPlaceId`).
    pub(crate) symbol: SymbolId,
    pub(crate) range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionKind {
    Assignment(AstPtr<RBinaryExpression>),
    SuperAssignment(AstPtr<RBinaryExpression>),
    Parameter(AstPtr<RParameter>),
    ForVariable(AstPtr<RForStatement>),
    /// A forwarding binding that resolves to a definition in another file.
    /// Consumers must chase the `file`/`name` chain to reach the actual origin.
    Import {
        call: AstPtr<RCall>,
        file: Url,
        name: String,
    },
    /// A binding created by a call (e.g. `assign("x", value)`) or a binding
    /// operator (`x %<>% f()`) rather than a syntactic `<-`. `node` is the whole
    /// binding expression (for its range and provenance), `name` the name
    /// argument or left operand (goto, rename), and `value` the value a type
    /// checker infers from (`None` when absent).
    Assign {
        node: AstPtr<AnyRExpression>,
        name: AstPtr<AnyRExpression>,
        value: Option<AstPtr<AnyRExpression>>,
    },
}

impl Definition {
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    pub fn kind(&self) -> &DefinitionKind {
        &self.kind
    }

    pub fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for Definition {
    fn range(&self) -> TextRange {
        self.range()
    }
}

// A site where a symbol is referenced by name. In ty, use sites are tracked
// via `ScopedUseId` indices in a per-scope `AstIds` structure (mapping AST
// node positions to use IDs). Our flat list serves the same purpose: the
// `UseDefMap` will reference `UseId` indices into this list to connect each
// use to its reaching definitions.
#[derive(Debug, PartialEq, Eq)]
pub struct Use {
    pub(crate) symbol: SymbolId,
    pub(crate) range: TextRange,
}

impl Use {
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    pub fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for Use {
    fn range(&self) -> TextRange {
        self.range()
    }
}

/// A cross-file call site recorded at parse time (`library()`,
/// `source()`, ...). Different kinds carry different downstream
/// semantics, see [`SemanticCallKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCall {
    pub(crate) kind: SemanticCallKind,
    pub(crate) offset: TextSize,
    pub(crate) scope: ScopeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticCallKind {
    /// `library(pkg)` or `require(pkg)`: attaches a package to the
    /// search path. Contributes a fallback layer for unbound symbols.
    Attach { package: String },
    /// `source("path")`: injects the sourced file's top-level
    /// bindings into the current scope. Local-scope semantics, not
    /// search-path semantics.
    ///
    /// `resolved` is the canonical URL the resolver mapped `path` to,
    /// or `None` if no resolver was provided or the resolver couldn't
    /// resolve the path.
    Source { path: String, resolved: Option<Url> },
}

impl SemanticCall {
    pub fn kind(&self) -> &SemanticCallKind {
        &self.kind
    }

    pub fn offset(&self) -> TextSize {
        self.offset
    }

    pub fn scope(&self) -> ScopeId {
        self.scope
    }
}

/// Namespace access recorded during indexing, i.e. `package::symbol` or
/// `package:::symbol`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceAccess {
    pub(crate) package: String,
    pub(crate) symbol: String,
    pub(crate) kind: NamespaceAccessKind,
    pub(crate) offset: TextSize,
}

impl NamespaceAccess {
    pub(crate) fn new(
        package: String,
        symbol: String,
        kind: NamespaceAccessKind,
        offset: TextSize,
    ) -> Self {
        Self {
            package,
            symbol,
            kind,
            offset,
        }
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn kind(&self) -> NamespaceAccessKind {
        self.kind
    }

    pub fn offset(&self) -> TextSize {
        self.offset
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceAccessKind {
    /// `::`
    Export,
    /// `:::`
    Internal,
}

/// A diagnostic surfaced while building the semantic index, for downstream
/// consumers to turn into user-facing diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticDiagnostic {
    /// An effectful call (NSE scope or attach) recognized in a lazy context
    /// whose callee is bound elsewhere with undetermined timing (later parent
    /// code, or another lazy context).
    LazyShadowAmbiguity { name: String, range: TextRange },
}

// --- Iterators ---

pub struct ChildScopeIdsIter<'a> {
    index: &'a SemanticIndex,
    current: ScopeId,
    end: ScopeId,
}

impl<'a> Iterator for ChildScopeIdsIter<'a> {
    type Item = ScopeId;

    fn next(&mut self) -> Option<ScopeId> {
        if self.current >= self.end {
            return None;
        }
        let id = self.current;
        // Skip over this child's descendants to get to the next sibling
        self.current = self.index.scopes[id].descendants.end;
        Some(id)
    }
}

pub struct AncestorScopeIdsIter<'a> {
    index: &'a SemanticIndex,
    current: Option<ScopeId>,
}

impl<'a> Iterator for AncestorScopeIdsIter<'a> {
    type Item = ScopeId;

    fn next(&mut self) -> Option<ScopeId> {
        let id = self.current?;
        self.current = self.index.scopes[id].parent;
        Some(id)
    }
}
