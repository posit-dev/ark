use std::ops::Range;
use std::sync::Arc;

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

    // Scope-chain directives called at top-level, such as `library()` or `require()`.
    directives: Vec<Directive>,
}

impl SemanticIndex {
    pub(crate) fn new(
        scopes: IndexVec<ScopeId, Scope>,
        symbol_tables: IndexVec<ScopeId, Arc<SymbolTable>>,
        definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
        uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
        use_def_maps: IndexVec<ScopeId, Arc<UseDefMap>>,
        enclosing_snapshots: FxHashMap<EnclosingSnapshotKey, (ScopeId, EnclosingSnapshotId)>,
        directives: Vec<Directive>,
    ) -> Self {
        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
            use_def_maps,
            enclosing_snapshots,
            directives,
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
    /// Last definition of each name wins (later assignments shadow earlier ones).
    pub fn file_exports(&self) -> FxHashMap<&str, TextRange> {
        let file_scope = ScopeId::from(0);
        let symbols = &self.symbol_tables[file_scope];

        let mut exports = FxHashMap::default();
        for (_id, def) in self.definitions[file_scope].iter() {
            let name = symbols.symbol(def.symbol()).name();
            exports.insert(name, def.range());
        }

        exports
    }

    /// Package names from `library()` / `require()` directives in this file.
    pub fn file_attached_packages(&self) -> Vec<&str> {
        self.directives
            .iter()
            .map(|d| match &d.kind {
                DirectiveKind::Attach(pkg) => pkg.as_str(),
            })
            .collect()
    }

    /// File-level directives (e.g. `library()` calls) recorded during indexing.
    pub fn file_directives(&self) -> &[Directive] {
        &self.directives
    }

    /// Find the innermost scope containing `offset`.
    pub fn scope_at(&self, offset: biome_rowan::TextSize) -> (ScopeId, &Scope) {
        // Start at the file scope
        let mut current = ScopeId::from(0);
        'outer: loop {
            for child_id in self.child_scopes(current) {
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

    /// Iterate direct child scopes of `scope`.
    pub fn child_scopes(&self, scope: ScopeId) -> ChildScopesIter<'_> {
        let descendants = &self.scopes[scope].descendants;
        ChildScopesIter {
            index: self,
            current: descendants.start,
            end: descendants.end,
        }
    }

    /// Walk from `scope` up through ancestors to the file root. Note that
    /// `scope` itself is included in the ancestors.
    pub fn ancestor_scopes(&self, scope: ScopeId) -> AncestorsIter<'_> {
        AncestorsIter {
            index: self,
            current: Some(scope),
        }
    }

    /// Resolve a name starting from `scope`, walking up the scope chain.
    pub fn resolve_symbol(&self, name: &str, scope: ScopeId) -> Option<(ScopeId, SymbolId)> {
        for ancestor in self.ancestor_scopes(scope) {
            if let Some(id) = self.symbol_tables[ancestor].id(name) {
                if self.symbol_tables[ancestor]
                    .symbol(id)
                    .flags()
                    .contains(SymbolFlags::IS_BOUND)
                {
                    return Some((ancestor, id));
                }
            }
        }
        None
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
///
/// When we implement NSE, we will add a `laziness: ScopeLaziness` field to
/// distinguish lazy snapshots (functions, accumulated union via watchers) from
/// eager snapshots (NSE scopes like `local()`, point-in-time capture at the
/// call site). Currently all nested scopes are lazy, so the field is omitted.
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
// Currently only `function()` creates a new scope. In the future, constructs
// like `local()`, `with()`, `within()` may also create scopes (determined
// by function declarations resolved via salsa queries).
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
// Definitions carry `file` and `scope` so they're self-contained when
// passed around (e.g., through `DefinitionKind::Import` chains during
// cross-file goto-definition). This mirrors ty's `Definition<'db>`, a
// salsa tracked struct that carries file + scope + place for the same
// reason.
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
    /// The file that owns this definition's index.
    // TODO(salsa): Should become a File.
    pub(crate) file: Url,
    /// The scope within the file's index where this definition lives.
    pub(crate) scope: ScopeId,
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

    pub fn file(&self) -> &Url {
        &self.file
    }

    pub fn scope(&self) -> ScopeId {
        self.scope
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

/// A directive that affects the file's imports (e.g. `library()` calls).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    pub(crate) kind: DirectiveKind,
    pub(crate) offset: TextSize,
    pub(crate) scope: ScopeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectiveKind {
    /// `library(pkg)` or `require(pkg)`: attaches a package to the search path.
    Attach(String),
}

impl Directive {
    pub fn kind(&self) -> &DirectiveKind {
        &self.kind
    }

    pub fn offset(&self) -> TextSize {
        self.offset
    }

    pub fn scope(&self) -> ScopeId {
        self.scope
    }
}

// --- Iterators ---

pub struct ChildScopesIter<'a> {
    index: &'a SemanticIndex,
    current: ScopeId,
    end: ScopeId,
}

impl<'a> Iterator for ChildScopesIter<'a> {
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

pub struct AncestorsIter<'a> {
    index: &'a SemanticIndex,
    current: Option<ScopeId>,
}

impl<'a> Iterator for AncestorsIter<'a> {
    type Item = ScopeId;

    fn next(&mut self) -> Option<ScopeId> {
        let id = self.current?;
        self.current = self.index.scopes[id].parent;
        Some(id)
    }
}
