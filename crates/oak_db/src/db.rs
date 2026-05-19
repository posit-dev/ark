use aether_url::UrlId;
use query_group_macro::query_group;
use rustc_hash::FxHashMap;

use crate::File;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::Package;
use crate::Root;
use crate::WorkspaceRoots;

/// Concrete-input surface of the salsa database. Each impl
/// ([`crate::OakDatabase`], the test db) supplies the three singleton input
/// handles.
///
/// Kept separate from [`Db`] (the query trait) so the `#[query_group]` macro on
/// `Db` doesn't try to interpret these accessor methods as salsa inputs.
/// Mirrors rust-analyzer's `SourceDatabase` / `DefDatabase` split: input
/// plumbing lives on the base trait, derived queries on the query-group trait.
#[salsa::db]
pub trait DbInputs: salsa::Database {
    /// Workspace folders opened by the editor.
    fn workspace_roots(&self) -> WorkspaceRoots;

    /// R library roots (entries in `.libPaths()`).
    fn library_roots(&self) -> LibraryRoots;

    /// Files not yet anchored to any workspace or library root.
    fn orphan_root(&self) -> OrphanRoot;
}

/// Salsa database trait used throughout `oak_db`. Tracked queries take `&dyn
/// Db`, so query code never names the concrete db type.
///
/// `#[query_group]` generates per-method shims plus a blanket impl covering
/// both `&dyn Db` and concrete db references, so the method call syntax (e.g.
/// `db.file_by_url(url)`) works in both contexts.
#[query_group]
pub trait Db: DbInputs {
    /// Look up the `File` interned at `url`, if any.
    ///
    /// Walks the per-root URL indices in workspace-then-library order,
    /// then falls back to the orphan bucket. The walk short-circuits
    /// on the first hit, so callers depend only on the index maps
    /// actually visited.
    #[salsa::invoke(file_by_url_query)]
    #[salsa::transparent]
    fn file_by_url(&self, url: &UrlId) -> Option<File>;

    /// Look up the `Package` named `name`, applying the following precedence:
    /// - Workspace packages shadow installed ones
    /// - Installed packages in an earlier root shadow later ones
    ///   (mirroring `.libPaths()`).
    #[salsa::invoke(package_by_name_query)]
    #[salsa::transparent]
    fn package_by_name(&self, name: &str) -> Option<Package>;
}

/// Implementation of [`Db::file_by_url`]. Walks the per-root indices.
///
/// Not itself salsa-tracked (its `&UrlId` argument isn't a salsa
/// entity), but every step is: each [`root_url_index`] call returns a
/// cached map, so adding a file to one root invalidates only that
/// root's index.
fn file_by_url_query(db: &dyn Db, url: &UrlId) -> Option<File> {
    for root in db.workspace_roots().roots(db) {
        if let Some(&file) = root_url_index(db, *root).get(url) {
            return Some(file);
        }
    }
    for root in db.library_roots().roots(db) {
        if let Some(&file) = root_url_index(db, *root).get(url) {
            return Some(file);
        }
    }
    orphan_url_index(db).get(url).copied()
}

/// Implementation of [`Db::package_by_name`]. Same shape as
/// [`file_by_url_query`].
fn package_by_name_query(db: &dyn Db, name: &str) -> Option<Package> {
    for root in db.workspace_roots().roots(db) {
        if let Some(&pkg) = root_package_index(db, *root).get(name) {
            return Some(pkg);
        }
    }
    for root in db.library_roots().roots(db) {
        if let Some(&pkg) = root_package_index(db, *root).get(name) {
            return Some(pkg);
        }
    }
    None
}

/// Per-root URL -> File index. Salsa caches one map per `Root`;
/// reads only `root.scripts`, `root.packages`, and each
/// `pkg.files` reachable from this root. Adding or removing a file
/// in *this* root invalidates this entry; other roots stay cached.
#[salsa::tracked(returns(ref))]
fn root_url_index(db: &dyn Db, root: Root) -> FxHashMap<UrlId, File> {
    let mut map = FxHashMap::default();
    for &file in root.scripts(db) {
        map.insert(file.url(db).clone(), file);
    }
    for &pkg in root.packages(db) {
        for &file in pkg.files(db) {
            map.insert(file.url(db).clone(), file);
        }
    }
    map
}

/// Orphan URL -> File index. Reads only `orphan_root().files`.
#[salsa::tracked(returns(ref))]
fn orphan_url_index(db: &dyn Db) -> FxHashMap<UrlId, File> {
    let mut map = FxHashMap::default();
    for &file in db.orphan_root().files(db) {
        map.insert(file.url(db).clone(), file);
    }
    map
}

/// Per-root name -> Package index. Same granularity as
/// [`root_url_index`].
#[salsa::tracked(returns(ref))]
fn root_package_index(db: &dyn Db, root: Root) -> FxHashMap<String, Package> {
    let mut map = FxHashMap::default();
    for &pkg in root.packages(db) {
        map.insert(pkg.name(db).clone(), pkg);
    }
    map
}
