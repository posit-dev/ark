use crate::Files;
use crate::LibraryRoots;
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

    /// URL-keyed `File` interner.
    fn files(&self) -> &Files;
}
