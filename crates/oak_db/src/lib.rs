use oak_semantic::library::Library;
use oak_semantic::semantic_index::SemanticIndex;
use url::Url;

/// Database trait for cross-file queries.
///
/// This will become a Salsa `#[salsa::db]` trait. For now it's a plain
/// trait that abstracts over how other files' semantic indexes are obtained.
pub trait Db {
    /// TODO(salsa): With tracked file inputs this becomes infallible in
    /// practice. `None` means the file disappeared between index-build
    /// and query time, which is an edge case, not a normal path.
    fn semantic_index(&self, file: &Url) -> Option<SemanticIndex>;
    fn library(&self) -> &Library;
}
