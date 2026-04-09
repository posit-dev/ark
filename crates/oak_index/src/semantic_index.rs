use std::ops::Range;

use biome_rowan::TextRange;
use rustc_hash::FxHashMap;

use crate::arena::define_index;
use crate::arena::IndexVec;

// File-local scope identifier
define_index!(ScopeId);

// Scope-local symbol identifier
define_index!(SymbolId);

// Definition site identifier
define_index!(DefinitionId);

// Use site identifier
define_index!(UseId);

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
pub struct SemanticIndex {
    scopes: IndexVec<ScopeId, Scope>,

    // ty wraps per-scope tables in `Arc` so they can be returned from
    // individual salsa tracked queries (e.g. `place_table(db, scope) ->
    // Arc<PlaceTable>`). Salsa compares the returned `Arc` by `Eq` to skip
    // re-running downstream queries when a scope's table hasn't changed.
    // We defer that until salsa is introduced.
    symbol_tables: IndexVec<ScopeId, SymbolTable>,

    // Flat per-scope lists of definition and use sites. These support rename
    // and go-to-definition by letting us find all sites for a given symbol
    // without control-flow analysis.
    //
    // In ty, this role is filled by salsa-tracked `Definition<'db>` structs
    // and `AstIds`. When we introduce salsa, these lists may be restructured
    // to match.
    //
    // Use-def maps will layer on top of these lists, not replace them. A
    // use-def map tracks which definitions reach each use through control flow,
    // referencing `DefinitionId` and `UseId` indices into these arenas.
    definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
    uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
}

impl SemanticIndex {
    pub(crate) fn new(
        scopes: IndexVec<ScopeId, Scope>,
        symbol_tables: IndexVec<ScopeId, SymbolTable>,
        definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
        uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
    ) -> Self {
        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
        }
    }

    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id]
    }

    pub fn symbols(&self, scope: ScopeId) -> &SymbolTable {
        &self.symbol_tables[scope]
    }

    pub fn definitions(&self, scope: ScopeId) -> &IndexVec<DefinitionId, Definition> {
        &self.definitions[scope]
    }

    pub fn uses(&self, scope: ScopeId) -> &IndexVec<UseId, Use> {
        &self.uses[scope]
    }

    /// Find the innermost scope containing `offset`.
    pub fn scope_at(&self, offset: biome_rowan::TextSize) -> ScopeId {
        // Start at the file scope
        let mut current = ScopeId::from(0);
        'outer: loop {
            for child_id in self.child_scopes(current) {
                if self.scopes[child_id].range.contains(offset) {
                    current = child_id;
                    continue 'outer;
                }
            }
            return current;
        }
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
                    .flags
                    .contains(SymbolFlags::IS_BOUND)
                {
                    return Some((ancestor, id));
                }
            }
        }
        None
    }
}

// --- Scope ---

#[derive(Debug)]
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

// --- Symbol table (per scope) ---

// Read-only after construction. The builder uses `SymbolTableBuilder`.
#[derive(Debug, Default)]
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

#[derive(Debug)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolFlags(u8);

impl SymbolFlags {
    // Referenced by name in this scope.
    pub const IS_USED: Self = Self(1 << 0);
    // Given a value: assignment (`<-`, `=`, `->`) or parameter definition.
    pub const IS_BOUND: Self = Self(1 << 1);
    // Appears in a function's formal parameter list.
    pub const IS_PARAMETER: Self = Self(1 << 2);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    Assignment,
    SuperAssignment,
    Parameter,
    ForVariable,
}

#[derive(Debug)]
pub struct Definition {
    pub(crate) symbol: SymbolId,
    pub(crate) kind: DefinitionKind,
    pub(crate) range: TextRange,
}

impl Definition {
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    pub fn kind(&self) -> DefinitionKind {
        self.kind
    }

    pub fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Debug)]
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
