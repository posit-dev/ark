use aether_url::UrlId;
use oak_package_metadata::namespace::Namespace;

use crate::root::url_to_root;
use crate::Db;
use crate::File;
use crate::Name;
use crate::Root;

/// Storage for the source graph. The edges (dependencies between nodes) are
/// encoded in `Script` and `Package` nodes.
///
/// - Scripts can depend on installed and workspace packages via e.g. `::`
///   or `library()`. They can also depend on other scripts via `source()`.
/// - Packages can import other packages, but do not depend on scripts.
#[salsa::input(singleton)]
pub struct SourceGraph {
    /// Scripts in the user workspace.
    #[returns(ref)]
    pub scripts: Vec<Script>,
    /// Workspace packages live in the user's workspace and are authoritative
    /// over installed packages. We always have full sources for workspace
    /// packages.
    #[returns(ref)]
    pub workspace_packages: Vec<Package>,
    /// Installed packages live in `.libPaths()`. They start out as stubs (no
    /// source files) and get updated by the LSP layer as sources become
    /// available via `oak_sources`.
    #[returns(ref)]
    pub installed_packages: Vec<Package>,
}

#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum SourceNode {
    Script(Script),
    Package(Package),
}

/// The set of workspace folders the user has open.
///
/// Populated by the LSP layer from `initialize.workspaceFolders` and
/// updated on `workspace/didChangeWorkspaceFolders`. Read by
/// [`File::workspace_root`](crate::File::workspace_root) to find the
/// containing workspace folder for a given URL, which `DbResolver`
/// uses as the anchor for relative `source("path")` arguments.
#[salsa::input(singleton)]
pub struct WorkspaceRoots {
    #[returns(ref)]
    pub roots: Vec<Root>,
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
    // from a basename spec via `Root.revision` and a `Files` registry.
    #[returns(ref)]
    pub collation: Vec<File>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PackageOrigin {
    Workspace { root: Root },
    Installed { version: String, libpath: UrlId },
}

impl SourceGraph {
    /// Construct an empty `SourceGraph` with no scripts or packages.
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, vec![], vec![], vec![])
    }
}

#[salsa::tracked]
impl SourceGraph {
    /// Look up a `Script` by URL.
    ///
    /// O(1) via [`Files`](crate::Files). Reads `root.revision(db)` for the
    /// URL's containing workspace root, or `WorkspaceRoots.roots` for orphan
    /// URLs, so callers in tracked queries get a salsa dependency that
    /// invalidates when files are added or removed.
    pub(crate) fn script_by_url(self, db: &dyn Db, url: &UrlId) -> Option<Script> {
        // Anchor on a salsa input that bumps when files are added to
        // or removed from this root. Without it, the only dependency is
        // on `file.parent`, which doesn't fire on `Files::intern` /
        // `Files::remove`. A cached `None` would survive a new file
        // being interned, and a cached `Some(s)` would survive the
        // file being removed.
        match url_to_root(db, url) {
            Some(root) => {
                let _ = root.revision(db);
            },
            None => {
                let _ = db.workspace_roots().roots(db);
            },
        }

        let file = db.files().get(url)?;
        match file.parent(db)? {
            SourceNode::Script(s) => Some(s),
            SourceNode::Package(_) => None,
        }
    }

    /// Look up a `Package` by name. Workspace packages take precedence over
    /// installed packages of the same name.
    #[salsa::tracked]
    pub fn package_by_name(self, db: &dyn Db, name: Name<'_>) -> Option<Package> {
        let text = name.text(db);
        self.workspace_packages(db)
            .iter()
            .chain(self.installed_packages(db).iter())
            .find(|package| package.name(db) == text)
            .copied()
    }
}
