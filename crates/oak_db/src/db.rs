use aether_url::UrlId;

use crate::File;
use crate::Files;
use crate::LibraryRoots;
use crate::Package;
use crate::Packages;
use crate::Script;
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

    /// URL-keyed `File` interner. Concrete-db storage detail; consumers
    /// should prefer the lookup methods below.
    fn files(&self) -> &Files;

    /// `(Root, name)` interner of `Package`s. Concrete-db storage
    /// detail, consumers should prefer the lookup methods below.
    fn packages(&self) -> &Packages;

    /// Look up the `File` interned at `url`, if any.
    ///
    /// Auto-anchors so tracked-query callers re-run when a file is
    /// interned at or removed from `url`. See [`Files::get`] for the
    /// underlying dependency-recording logic.
    fn file_by_url(&self, url: &UrlId) -> Option<File> {
        self.files().get(self, url)
    }

    /// Look up the `Script` interned at `url`. Returns `None` if no
    /// file is interned at `url`, if the file has no owner, or if the
    /// owner is a `Package` (i.e., the file is inside a package, not a
    /// standalone script). Inherits the auto-anchoring of
    /// [`Self::file_by_url`].
    fn script_by_url(&self, url: &UrlId) -> Option<Script> {
        self.files().get_script(self, url)
    }

    /// Look up the `Package` named `name`, applying R's precedence:
    /// workspace packages shadow installed ones; within each group,
    /// declaration order wins. Anchors lazily on each root walked.
    fn package_by_name(&self, name: &str) -> Option<Package> {
        self.packages().get(self, name)
    }
}
