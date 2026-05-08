use oak_semantic::semantic_index::SemanticIndex;
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
    #[salsa::tracked(returns(ref))]
    pub(crate) fn parse(self, db: &dyn Db) -> OakParse {
        OakParse::new(aether_parser::parse(
            self.contents(db),
            aether_parser::RParserOptions::default(),
        ))
    }

    /// Build this file's `SemanticIndex` from the parse tree.
    ///
    /// Crate-internal: cross-file consumers must use narrower public queries
    /// (`exports`, `external_scope`) to avoid depending on the full index,
    /// which would invalidate on every internal edit.
    #[salsa::tracked(returns(ref))]
    pub(crate) fn semantic_index(self, db: &dyn Db) -> SemanticIndex {
        let parsed = self.parse(db);
        oak_semantic::semantic_index(&parsed.tree(), self.url(db))
    }
}
