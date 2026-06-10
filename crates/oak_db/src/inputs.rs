use aether_url::UrlId;
use oak_package_metadata::namespace::Namespace;

use crate::Db;
use crate::File;

/// Salsa-tracked root directory.
///
/// May contain top-level R scripts (typically in a Workspace root) and
/// `Package`s (from a Workspace root or a Library root), which themselves
/// wrap R `File`s.
///
/// Watchers implemented in the consumer/LSP layer are reponsible for populating
/// and keeping in sync the packages and scripts in these roots (LSP file
/// watcher for `Workspace`, custom library watcher for `Library`).
///
/// The `scripts` and `packages` fields are the salsa-observable signal for "the
/// set of scripts/packages under this root changed": tracked queries that read
/// them are invalidated when the watcher updates the corresponding list.
#[salsa::input(debug)]
pub struct Root {
    #[returns(ref)]
    pub path: UrlId,
    pub kind: RootKind,
    /// Top-level R scripts directly under this root. Each entry is a
    /// `File` with `package(db) == None`. Always empty for `Library`
    /// roots.
    #[returns(ref)]
    pub scripts: Vec<File>,
    /// Packages discovered under this root (workspace packages for
    /// `Workspace`, installed packages for `Library`).
    #[returns(ref)]
    pub packages: Vec<Package>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RootKind {
    Workspace,
    Library,
}

/// The set of workspace folders the user has open.
///
/// Populated by the LSP layer from `initialize.workspaceFolders` and
/// updated on `workspace/didChangeWorkspaceFolders`. Each entry is a
/// [`Root`] of kind [`RootKind::Workspace`](crate::RootKind::Workspace).
#[salsa::input]
pub struct WorkspaceRoots {
    #[returns(ref)]
    pub roots: Vec<Root>,
}

impl WorkspaceRoots {
    /// Construct an empty `WorkspaceRoots` with no folders.
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, vec![])
    }
}

/// The set of R libraries (`.libPaths()` entries).
///
/// Populated by the library watcher (outside the LSP since libraries live
/// outside the user's project). Each entry is a [`Root`] of kind
/// [`RootKind::Library`](crate::RootKind::Library). Order matches R's
/// `.libPaths()` lookup order.
#[salsa::input]
pub struct LibraryRoots {
    #[returns(ref)]
    pub roots: Vec<Root>,
}

impl LibraryRoots {
    /// Construct an empty `LibraryRoots` with no library directories.
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, vec![])
    }
}

/// Files known to the database that aren't anchored to a workspace
/// or library root.
///
/// Holds: untitled buffers, files opened in the editor before the
/// workspace scanner has placed them, and any file whose URL falls
/// outside every workspace / library folder. Scanners may move files
/// out of this bucket into `Root.scripts` or `Package.files` once they
/// classify them.
///
/// Singleton: there is one `OrphanRoot` per concrete database, lazily
/// initialised by the implementation. The `files` field is what
/// [`crate::Db::file_by_url`] consults to find unanchored files.
#[salsa::input]
pub struct OrphanRoot {
    #[returns(ref)]
    pub files: Vec<File>,
}

impl OrphanRoot {
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, vec![])
    }
}

#[salsa::input(debug)]
pub struct Package {
    /// The `Root` this package belongs to. Workspace packages live under
    /// a [`RootKind::Workspace`] root, installed packages live under a
    /// [`RootKind::Library`] root. Read `root.kind(db)` to distinguish.
    pub root: Root,
    #[returns(ref)]
    pub name: String,
    /// Installed-package version (from `DESCRIPTION`). `None` for
    /// workspace packages.
    #[returns(ref)]
    pub version: Option<String>,
    #[returns(ref)]
    pub namespace: Namespace,
    /// R source files belonging to this package (the `R/*.R` files).
    /// Per-package granularity: adding or removing a file in one
    /// package doesn't invalidate tracked queries reading another
    /// package's files.
    #[returns(ref)]
    pub files: Vec<File>,
    /// The basename ordering from `DESCRIPTION`'s `Collate` field, if
    /// present. `None` when the field is absent (R defaults to
    /// alphabetical load order). Changes only when `DESCRIPTION`
    /// itself changes, so this anchor is independent of `files` (which
    /// bumps when R/ files are added or removed).
    #[returns(ref)]
    pub collation: Option<Vec<String>>,
}
