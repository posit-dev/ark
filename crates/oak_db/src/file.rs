use std::sync::Arc;

use aether_url::UrlId;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolTable;
use oak_semantic::use_def_map::UseDefMap;

use crate::parse::OakParse;
use crate::resolver::DbResolver;
use crate::Db;

/// A source file tracked by Salsa.
///
/// Content is pushed into Salsa by the LSP layer, the database never does I/O.
/// This matches rust-analyzer's push model and avoids tying parsing to
/// disk/network I/O inside a Salsa query.
///
/// The `url` field is a [`UrlId`], so the type system enforces "everything
/// inside Salsa is a canonical URL".
#[salsa::input(debug)]
pub struct File {
    #[returns(ref)]
    pub url: UrlId,
    #[returns(ref)]
    pub contents: String,
}

#[salsa::tracked]
impl File {
    /// Parse this file's contents into an R syntax tree.
    ///
    /// Crate-internal: kept `pub(crate)` so downstream crates can't take a
    /// dependency on a full parse tree. They reach the tree only through
    /// narrower public queries.
    ///
    /// `lru = 128` caps the number of live parse trees to 128. Matches
    /// rust-analyzer's default for its analogous `parse(file_id)` query.
    /// Rowan's green tree shares structure across edits, so eviction frees
    /// memory cleanly. Derived queries (e.g. `semantic_index`) store
    /// `AstPtr`s rather than tree nodes, so they don't pin an evicted tree.
    #[salsa::tracked(returns(ref), lru = 128)]
    pub(crate) fn parse(self, db: &dyn Db) -> OakParse {
        OakParse::new(aether_parser::parse(
            self.contents(db),
            aether_parser::RParserOptions::default(),
        ))
    }

    /// Build this file's `SemanticIndex` from the parse tree.
    ///
    /// `pub(crate)` so [`DbResolver`] and tests can reach it. External
    /// consumers should go through the narrow tracked queries below.
    ///
    /// TODO(salsa): tighten back to private once narrow cross-file
    /// queries land (`file_exports`, `file_attached_packages`) and
    /// `DbResolver::resolve_source` reads those instead of the full
    /// index. The privacy reverts to file-local + `cfg(test)` for
    /// tests at that point.
    ///
    /// Cross-file symbol resolution (`source()` injection, NSE resolution)
    /// is driven by [`DbResolver`]. `cycle_result` recovers from cyclic
    /// `source()` chains by returning an empty index for whichever side
    /// salsa picks to break the cycle. R doesn't allow `A` sources `B`
    /// sources `A`, so precision loss is acceptable.
    ///
    /// `no_eq` skips salsa's `values_equal` check after recomputation.
    /// Backdating at this level never triggered in practice anyway: `AstPtr`
    /// ranges inside `Definition`s typically shift on edits.
    #[salsa::tracked(returns(ref), no_eq, cycle_result = semantic_index_cycle_result)]
    pub(crate) fn semantic_index(self, db: &dyn Db) -> SemanticIndex {
        build_semantic_index(self, db)
    }

    /// The symbol table for one scope of this file.
    #[salsa::tracked]
    pub fn symbol_table(self, db: &dyn Db, scope: ScopeId) -> Arc<SymbolTable> {
        Arc::clone(self.semantic_index(db).symbols(scope))
    }

    /// The use-def map for one scope of this file.
    #[salsa::tracked]
    pub fn use_def_map(self, db: &dyn Db, scope: ScopeId) -> Arc<UseDefMap> {
        Arc::clone(self.semantic_index(db).use_def_map(scope))
    }
}

fn build_semantic_index(file: File, db: &dyn Db) -> SemanticIndex {
    let parsed = file.parse(db);
    let mut resolver = DbResolver::new(db, file);
    oak_semantic::build_index(&parsed.tree(), file.url(db).as_url(), &mut resolver)
}

fn semantic_index_cycle_result(_db: &dyn Db, _id: salsa::Id, _file: File) -> SemanticIndex {
    SemanticIndex::empty()
}
