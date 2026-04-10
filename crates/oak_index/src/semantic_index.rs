use std::ops::Range;

use aether_syntax::RSyntaxNode;
use biome_rowan::TextRange;
use rustc_hash::FxHashMap;

use crate::index_vec::define_index;
use crate::index_vec::IndexVec;

// File-local scope identifier
define_index!(ScopeId);

// Scope-local symbol identifier
define_index!(SymbolId);

// Scope-local definition site identifier
define_index!(DefinitionId);

// Scope-local use site identifier
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
    // In ty, definitions are salsa-tracked `Definition<'db>` structs (stored
    // in `definitions_by_node`), and use sites are tracked via `AstIds`
    // (a per-scope map from AST node positions to `ScopedUseId`). When we
    // introduce salsa, these lists may be restructured to match.
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

// A lexical scope within a file. Each scope has its own symbol table,
// definitions, and uses. The scope chain (via `parent`) is always bounded
// by the file: walking `parent` from any scope eventually reaches the
// `File` scope which itself has `parent: None`.
//
// Currently only `function()` creates a new scope. In the future, constructs
// like `local()`, `with()`, `within()` may also create scopes (determined
// by function declarations resolved via salsa queries).
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
//
// These flags are scope-level summaries, not fine-grained enough to
// implement LSP features directly. For example, `IS_BOUND` says "x is
// bound somewhere in this scope" but can't answer "which definition of x
// reaches this point?" or "is x defined before this use?". Use-def maps
// provide that precision. The flags remain useful for scope-level queries
// like `resolve_symbol` and `resolve_super_target` (which walk the scope
// chain checking `IS_BOUND`). They can also be useful as filters for
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
// Our definitions don't carry file or scope because they live inside the
// `SemanticIndex` at a known position (`definitions[scope_id][def_id]`).
// In ty, `Definition<'db>` is a self-contained salsa tracked struct that
// carries file + scope + place, because it gets passed around independently
// to type inference queries and cross-file lookups. We'll add those fields
// when salsa is introduced.
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
#[derive(Debug)]
pub struct Definition {
    pub(crate) symbol: SymbolId,
    pub(crate) kind: DefinitionKind,
    pub(crate) range: TextRange,
}

#[derive(Debug, Clone)]
pub enum DefinitionKind {
    Assignment(RSyntaxNode),
    SuperAssignment(RSyntaxNode),
    Parameter(RSyntaxNode),
    ForVariable(RSyntaxNode),
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

// A site where a symbol is referenced by name. In ty, use sites are tracked
// via `ScopedUseId` indices in a per-scope `AstIds` structure (mapping AST
// node positions to use IDs). Our flat list serves the same purpose: the
// `UseDefMap` will reference `UseId` indices into this list to connect each
// use to its reaching definitions.
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
