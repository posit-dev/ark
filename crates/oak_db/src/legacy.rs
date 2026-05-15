use oak_semantic::library::Library;
use oak_semantic::semantic_index::SemanticIndex;
use url::Url;

/// Legacy database trait used by the tree-sitter backed code path.
///
/// Exists only during the Salsa migration.
pub trait LegacyDb {
    /// `None` means the file disappeared between index-build and query time,
    /// which is an edge case, not a normal path. With Salsa inputs this
    /// becomes infallible.
    fn semantic_index(&self, file: &Url) -> Option<SemanticIndex>;
    fn library(&self) -> &Library;
}
