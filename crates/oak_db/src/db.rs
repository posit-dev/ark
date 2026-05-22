use aether_url::UrlId;
use rustc_hash::FxHashMap;

use crate::File;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::Package;
use crate::Root;
use crate::StaleRoot;
use crate::WorkspaceRoots;

/// Concrete-input surface of the salsa database. Each impl
/// ([`crate::OakDatabase`], the test db) supplies the three singleton input
/// handles.
///
/// Kept separate from [`Db`] (the query trait) so input accessors and derived
/// queries live on different traits. Mirrors rust-analyzer's `SourceDatabase`
/// / `DefDatabase` split: input plumbing on the base trait, derived queries
/// on the query trait.
#[salsa::db]
pub trait DbInputs: salsa::Database {
    /// Workspace folders opened by the editor.
    fn workspace_roots(&self) -> WorkspaceRoots;

    /// R library roots (entries in `.libPaths()`).
    fn library_roots(&self) -> LibraryRoots;

    /// Files not yet anchored to any workspace or library root.
    fn orphan_root(&self) -> OrphanRoot;

    /// Files and packages from roots that have been removed. Holding
    /// pen for entity reuse on re-add (see [`StaleRoot`]).
    fn stale_root(&self) -> StaleRoot;
}

/// Salsa database trait used throughout `oak_db`. Tracked queries take `&dyn
/// Db`, so query code never names the concrete db type.
///
/// Methods aren't memoized at this level: they delegate to free helpers
/// (`file_by_url_query` etc.) that walk per-root indices which *are* memoized,
/// so salsa records dep edges through those.
///
/// Each concrete db type provides its own forwarding `impl Db`, which is
/// what lets `db.file_by_url(url)` work on both `&dyn Db` (via the trait
/// method) and concrete db references (via the type's impl).
#[salsa::db]
pub trait Db: DbInputs {
    /// Look up the `File` interned at `url`, if any.
    ///
    /// Walks the per-root URL indices in workspace-then-library order,
    /// then falls back to the orphan bucket. The walk short-circuits
    /// on the first hit, so callers depend only on the index maps
    /// actually visited.
    fn file_by_url(&self, url: &UrlId) -> Option<File>;

    /// Look up the `Package` named `name`, applying the following precedence:
    /// - Workspace packages shadow installed ones
    /// - Installed packages in an earlier root shadow later ones
    ///   (mirroring `.libPaths()`).
    fn package_by_name(&self, name: &str) -> Option<Package>;

    /// Look up a `Package` by its `DESCRIPTION` URL.
    ///
    /// Walks workspace packages, then library packages, then falls back
    /// to [`StaleRoot`]. Stale matches are intentional: scanner upserts
    /// use this to find a `Package` entity whose live container was
    /// dropped on a previous `set_*_paths` call, so the entity gets
    /// reused on re-add. Analysis paths should not call this — they use
    /// [`Db::package_by_name`] which is stale-blind.
    fn package_by_url(&self, url: &UrlId) -> Option<Package>;
}

/// Implementation of [`Db::file_by_url`]. Walks the per-root indices.
///
/// Not itself salsa-tracked (its `&UrlId` argument isn't a salsa
/// entity), but every step is: each [`root_url_index`] call returns a
/// cached map, so adding a file to one root invalidates only that
/// root's index.
pub fn file_by_url_query(db: &dyn Db, url: &UrlId) -> Option<File> {
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
pub fn package_by_name_query(db: &dyn Db, name: &str) -> Option<Package> {
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

/// Implementation of [`Db::package_by_url`]. Walks live roots' packages
/// by `description_url`, then falls back to the stale bucket.
pub fn package_by_url_query(db: &dyn Db, url: &UrlId) -> Option<Package> {
    for root in db.workspace_roots().roots(db) {
        if let Some(&pkg) = root_package_url_index(db, *root).get(url) {
            return Some(pkg);
        }
    }
    for root in db.library_roots().roots(db) {
        if let Some(&pkg) = root_package_url_index(db, *root).get(url) {
            return Some(pkg);
        }
    }
    stale_package_url_index(db).get(url).copied()
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

/// Per-root DESCRIPTION URL -> Package index. Used by
/// [`package_by_url_query`] for entity-reuse lookups across rescans;
/// salsa cache invalidates only when this root's packages change.
#[salsa::tracked(returns(ref))]
fn root_package_url_index(db: &dyn Db, root: Root) -> FxHashMap<UrlId, Package> {
    let mut map = FxHashMap::default();
    for &pkg in root.packages(db) {
        map.insert(pkg.description_url(db).clone(), pkg);
    }
    map
}

/// Stale file URL -> File index. Reads only `stale_root().files`. Not
/// consulted by [`file_by_url_query`] — analysis is stale-blind by
/// design. Scanner upserts use [`stale_file_by_url`] when re-adding a
/// path.
#[salsa::tracked(returns(ref))]
fn stale_url_index(db: &dyn Db) -> FxHashMap<UrlId, File> {
    let mut map = FxHashMap::default();
    for &file in db.stale_root().files(db) {
        map.insert(file.url(db).clone(), file);
    }
    map
}

/// Look up a stale `File` by URL. Public so scanner upsert helpers in
/// `oak_scan` can fall back to stale after [`Db::file_by_url`] misses.
pub fn stale_file_by_url(db: &dyn Db, url: &UrlId) -> Option<File> {
    stale_url_index(db).get(url).copied()
}

/// Stale DESCRIPTION URL -> Package index. Same role as
/// [`stale_url_index`] for packages.
#[salsa::tracked(returns(ref))]
fn stale_package_url_index(db: &dyn Db) -> FxHashMap<UrlId, Package> {
    let mut map = FxHashMap::default();
    for &pkg in db.stale_root().packages(db) {
        map.insert(pkg.description_url(db).clone(), pkg);
    }
    map
}
