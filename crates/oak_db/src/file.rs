use std::sync::Arc;

use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolTable;
use oak_semantic::use_def_map::UseDefMap;
use url::Url;

use crate::parse::OakParse;
use crate::Db;

/// A source file tracked by Salsa.
///
/// Content is pushed into Salsa by the LSP layer, the database never does I/O.
/// This matches rust-analyzer's push model and avoids tying parsing to
/// disk/network I/O inside a Salsa query.
#[salsa::input]
pub struct File {
    #[returns(ref)]
    pub url: Url,
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
    /// Private to this file to prevent coarse Salsa queries. Consumers should
    /// go through the narrow tracked queries below.
    ///
    /// The full index re-runs typically on every edit (e.g. the `AstPtr` ranges
    /// inside `Definition`s shift, so it rarely backdates).
    #[cfg(not(test))]
    #[salsa::tracked(returns(ref), no_eq)]
    fn semantic_index(self, db: &dyn Db) -> SemanticIndex {
        build_semantic_index(self, db)
    }

    /// Tests use the `pub(crate)` variant gated behind `cfg(test)` so they can
    /// call into `semantic_index` directly to verify salsa caching behaviour.
    #[cfg(test)]
    #[salsa::tracked(returns(ref), no_eq)]
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
    oak_semantic::semantic_index(&parsed.tree(), file.url(db))
}
