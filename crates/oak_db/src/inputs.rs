use aether_url::UrlId;
use oak_package_metadata::namespace::Namespace;

use crate::Db;
use crate::File;
use crate::Name;

/// Salsa-tracked root directory.
///
/// May contain `Script`s (typically in a Workspace root) and `Package`s (from a
/// Workspace root or a Library root), which themselves wrap R `File`s.
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
    /// Top-level R scripts directly under this root. Always empty for
    /// `Library` roots.
    #[returns(ref)]
    pub scripts: Vec<Script>,
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

#[salsa::input(debug)]
pub struct Script {
    /// The `Root` this script belongs to. Always [`RootKind::Workspace`].
    pub root: Root,
    pub file: File,
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

/// Look up the workspace root that contains `url`, longest-prefix among
/// nested roots.
///
/// Returns `None` for non-`file:` URLs and for URLs that don't lie under
/// any workspace folder. Walks [`WorkspaceRoots`] linearly.
///
/// TODO(salsa, PR 8): becomes a `#[salsa::tracked]` function (so each
/// caller doesn't redo the prefix walk) and likely moves under the
/// `Files` interner as `Files::root_by_url`.
pub fn root_by_url(db: &dyn Db, url: &UrlId) -> Option<Root> {
    let path = url.to_file_path()?;
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter_map(|root| {
            let root_path = root.path(db).to_file_path()?;
            path.starts_with(&root_path).then_some((root_path, *root))
        })
        .max_by_key(|(p, _)| p.components().count())
        .map(|(_, r)| r)
}

/// Look up a `Package` by name. Walks workspace roots in declaration
/// order first (so workspace packages shadow installed packages of the
/// same name), then library roots in `.libPaths()` order.
///
/// Salsa records dependencies only on the roots actually walked: if the
/// match is in `workspace_roots[0]`, only that root's `packages` field
/// is read, so adding a same-name package in a lower-precedence library
/// root won't invalidate the result.
///
/// TODO(salsa, PR 8): delete this function. The precedence walk moves
/// inside `Packages::get(db, name)` on the interner, and callers use
/// `db.packages().get(db, name)` directly.
pub fn package_by_name(db: &dyn Db, name: Name<'_>) -> Option<Package> {
    let text = name.text(db);
    for root in db.workspace_roots().roots(db) {
        if let Some(pkg) = root.packages(db).iter().find(|p| p.name(db) == text) {
            return Some(*pkg);
        }
    }
    for root in db.library_roots().roots(db) {
        if let Some(pkg) = root.packages(db).iter().find(|p| p.name(db) == text) {
            return Some(*pkg);
        }
    }
    None
}
