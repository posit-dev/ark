use aether_syntax::RRoot;
use oak_semantic::library::Library;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;
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

/// Build a [`SemanticIndex`] with cross-file `source()` resolution
/// driven by a `FnMut` callback.
///
/// Used by `ark::lsp::state::WorldState`'s tree-sitter backed
/// resolver, which closes over LSP state (workspace document map,
/// cycle-detection stack) to satisfy [`ImportsResolver`] without
/// defining a struct.
///
/// TODO(salsa): retire alongside the LSP migration to oak_db's
/// `File::semantic_index` tracked query. Once `ark::lsp::state`
/// switches to consuming the tracked query (or implements its own
/// [`ImportsResolver`] directly), this function and its private
/// [`CallbackResolver`] adapter can be deleted.
pub fn semantic_index_with_source_resolver(
    root: &RRoot,
    file: &Url,
    resolver: impl FnMut(&str) -> Option<SourceResolution>,
) -> SemanticIndex {
    let mut resolver = CallbackResolver(resolver);
    oak_semantic::build_index(root, file, &mut resolver)
}

struct CallbackResolver<F>(F)
where
    F: FnMut(&str) -> Option<SourceResolution>;

impl<F> ImportsResolver for CallbackResolver<F>
where
    F: FnMut(&str) -> Option<SourceResolution>,
{
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        (self.0)(path)
    }
}
