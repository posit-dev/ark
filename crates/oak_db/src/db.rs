use aether_url::UrlId;

use crate::LibraryRoots;
use crate::Root;
use crate::WorkspaceRoots;

/// Salsa Database trait.
///
/// Queries take a `dyn Db` rather than the concrete database owned by
/// the LSP layer.
///
/// `WorkspaceRoots` and `LibraryRoots` are meant to be singletons. Concrete dbs
/// lazy-init these inputs via e.g. `Arc<OnceLock<_>>`. The `WorkspaceRoots`
/// list is typically updated by the LSP layer (workspace notification) whereas
/// `LibraryRoots` is updated by a library watcher.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Workspace folders opened by the editor.
    fn workspace_roots(&self) -> WorkspaceRoots;

    /// R library roots (entries in `.libPaths()`).
    fn library_roots(&self) -> LibraryRoots;

    /// Look up the workspace root that contains `url`, longest-prefix
    /// among nested roots.
    ///
    /// Returns `None` for non-`file:` URLs and for URLs that don't lie under
    /// any workspace folder. Walks [`WorkspaceRoots`] linearly.
    fn root_by_url(&self, url: &UrlId) -> Option<Root> {
        let path = url.to_file_path()?;
        self.workspace_roots()
            .roots(self)
            .iter()
            .filter_map(|root| {
                let root_path = root.path(self).to_file_path()?;
                path.starts_with(&root_path).then_some((root_path, *root))
            })
            .max_by_key(|(p, _)| p.components().count())
            .map(|(_, r)| r)
    }
}
