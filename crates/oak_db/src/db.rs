use aether_url::UrlId;
use rustc_hash::FxHashMap;

use crate::File;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::Package;
use crate::Root;
use crate::WorkspaceRoots;

/// Salsa database trait. Tracked queries take `&dyn Db`, so query
/// code never names the concrete db type ([`crate::OakDatabase`]).
///
/// [`WorkspaceRoots`], [`LibraryRoots`], and [`OrphanRoot`] are
/// singletons per database.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Workspace folders opened by the editor.
    fn workspace_roots(&self) -> WorkspaceRoots;

    /// R library roots (entries in `.libPaths()`).
    fn library_roots(&self) -> LibraryRoots;

    /// Files not yet anchored to any workspace or library root.
    fn orphan_root(&self) -> OrphanRoot;

    /// Look up the `File` interned at `url`, if any.
    ///
    /// Salsa-cached: the first call walks the per-root indices, the rest hit
    /// the cache until a relevant root's `scripts` / `packages` / per-package
    /// `files` (or `orphan_root().files`) changes.
    ///
    /// The `Self: Sized` bound allows the default implementation to dispatch to
    /// a salsa -racked function that takes `&dyn Db`.
    fn file_by_url(&self, url: &UrlId) -> Option<File>
    where
        Self: Sized,
    {
        file_by_url_query(self, url)
    }

    /// Look up the `Package` named `name`, applying the following precedence:
    /// - Workspace packages shadow installed ones
    /// - Installed packages in an earlier root shadow later one (mirroring `.libPaths()`).
    fn package_by_name(&self, name: &str) -> Option<Package>
    where
        Self: Sized,
    {
        package_by_name_query(self, name)
    }
}

/// Implementation of [`Db::file_by_url`]. Walks the per-root indices.
///
/// Not itself a salsa-tracked function (its `&UrlId` argument isn't a
/// salsa entity), but every step is: each [`root_url_index`] call
/// returns a cached map. Adding a file to one root invalidates only
/// that root's index. `pub(crate)` so `&dyn Db` callers inside the
/// crate (e.g. the resolver) can use it without the trait method's
/// `Self: Sized` bound; downstream code dispatches via the trait
/// method.
pub(crate) fn file_by_url_query(db: &dyn Db, url: &UrlId) -> Option<File> {
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
pub(crate) fn package_by_name_query(db: &dyn Db, name: &str) -> Option<Package> {
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
