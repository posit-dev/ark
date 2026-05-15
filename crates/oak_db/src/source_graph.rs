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

#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum FileOwner {
    Script(Script),
    Package(Package),
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
    pub file: File,
}

#[salsa::input(debug)]
pub struct Package {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub kind: PackageOrigin,
    #[returns(ref)]
    pub namespace: Namespace,
    // TODO(salsa): adding any `R/` file mutates this Vec and invalidates
    // every tracked query that read it. Future fix derives `Vec<File>`
    // from a basename spec via per-`Package` `files` and a `Files` registry.
    #[returns(ref)]
    pub collation: Vec<File>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PackageOrigin {
    Workspace { root: Root },
    Installed { version: String, libpath: UrlId },
}

/// Look up a `Script` by URL. Walks workspace roots looking for a script
/// whose file URL matches.
///
/// TODO(salsa, PR 8): delete this function. Once `Files` and `File.parent`
/// land, the only caller (`DbResolver::resolve_source`) inlines as
/// `db.files().get(db, url).and_then(|f| match f.parent(db) { ... })`.
pub fn script_by_url(db: &dyn Db, url: &UrlId) -> Option<Script> {
    for root in db.workspace_roots().roots(db) {
        if let Some(script) = root.scripts(db).iter().find(|s| s.file(db).url(db) == url) {
            return Some(*script);
        }
    }
    None
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
