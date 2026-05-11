use std::path::PathBuf;

use oak_package_metadata::namespace::Namespace;
use url::Url;

use crate::Db;
use crate::File;

#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum SourceNode {
    Script(Script),
    Package(Package),
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
    pub namespace: Namespace,
    #[returns(ref)]
    pub collation: Vec<File>,
    #[returns(ref)]
    pub kind: PackageOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PackageOrigin {
    Workspace { root: PathBuf },
    Installed { version: String, libpath: PathBuf },
}

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

/// Look up a `Script` by URL.
///
/// Plain function (not `#[salsa::tracked]`) because `Url` isn't indexable
/// without interning.
pub fn script_by_url(db: &dyn Db, url: &Url) -> Option<Script> {
    db.source_graph()
        .scripts(db)
        .iter()
        .find(|script| script.file(db).url(db) == url)
        .copied()
}

/// Look up a `Package` by name. Workspace packages take precedence over
/// installed packages of the same name.
pub fn package_by_name(db: &dyn Db, name: &str) -> Option<Package> {
    let source_graph = db.source_graph();
    find_named(db, source_graph.workspace_packages(db), name)
        .or_else(|| find_named(db, source_graph.installed_packages(db), name))
}

fn find_named(db: &dyn Db, packages: &[Package], name: &str) -> Option<Package> {
    packages
        .iter()
        .find(|package| package.name(db) == name)
        .copied()
}
