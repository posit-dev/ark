use aether_url::UrlId;
use rustc_hash::FxHashMap;

use crate::File;
use crate::LibraryRoots;
use crate::LiveRoot;
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

    /// Resolve the live `Root` that contains `pkg`, if any.
    ///
    /// Returns `None` when the package is only in [`StaleRoot`] (its live
    /// container was previously evicted).
    ///
    /// **Nested roots.** Two roots can claim the same package when one is
    /// nested inside the other, e.g. the frontend opens both `/proj` and
    /// `/proj/sub-pkg` as workspace folders and both scans walk into
    /// `sub-pkg/DESCRIPTION`. Both scans hand the same `Package` entity to
    /// their respective root's `packages` vec; the longest-path root wins
    /// the ownership query here. The shorter root's vec still transiently
    /// lists the package, but it self-heals on its next scan since
    /// `set_packages` replaces the vec wholesale.
    fn root_by_package(&self, pkg: Package) -> Option<Root>;

    /// All live roots in lookup-precedence order: workspace folders first, then
    /// library paths (mirroring R's `.libPaths()`), then the orphan bucket.
    /// Stale roots are not included. Salsa-cached and invalidates when one of
    /// `workspace_roots` / `library_roots` / `orphan_root` changes.
    fn live_roots(&self) -> &[LiveRoot];
}

#[salsa::tracked(returns(ref))]
pub fn live_roots_query(db: &dyn Db) -> Vec<LiveRoot> {
    let mut roots: Vec<LiveRoot> = db
        .workspace_roots()
        .roots(db)
        .iter()
        .map(|&r| LiveRoot::Workspace(r))
        .collect();

    roots.extend(
        db.library_roots()
            .roots(db)
            .iter()
            .map(|&r| LiveRoot::Library(r)),
    );

    roots.push(LiveRoot::Orphan(db.orphan_root()));
    roots
}

/// Implementation of [`Db::file_by_url`]. Walks the per-root indices.
///
/// Not itself salsa-tracked (its `&UrlId` argument isn't a salsa
/// entity), but every step is: each [`root_url_index`] call returns a
/// cached map, so adding a file to one root invalidates only that
/// root's index.
pub fn file_by_url_query(db: &dyn Db, url: &UrlId) -> Option<File> {
    for &root in db.live_roots() {
        let hit = match root {
            LiveRoot::Workspace(r) | LiveRoot::Library(r) => {
                root_url_index(db, r).get(url).copied()
            },
            LiveRoot::Orphan(_) => orphan_url_index(db).get(url).copied(),
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

/// Implementation of [`Db::package_by_name`]. Same shape as
/// [`file_by_url_query`]; orphan has no packages, so it contributes
/// nothing to the walk.
pub fn package_by_name_query(db: &dyn Db, name: &str) -> Option<Package> {
    for &root in db.live_roots() {
        if let LiveRoot::Workspace(r) | LiveRoot::Library(r) = root {
            if let Some(&pkg) = root_package_index(db, r).get(name) {
                return Some(pkg);
            }
        }
    }
    None
}

/// Implementation of [`Db::package_by_url`]. Walks live roots' packages
/// by `description_url`, then falls back to the stale bucket.
pub fn package_by_url_query(db: &dyn Db, url: &UrlId) -> Option<Package> {
    for &root in db.live_roots() {
        if let LiveRoot::Workspace(r) | LiveRoot::Library(r) = root {
            if let Some(&pkg) = root_package_url_index(db, r).get(url) {
                return Some(pkg);
            }
        }
    }
    stale_package_url_index(db).get(url).copied()
}

/// Implementation of [`Db::root_by_package`]. Walks all live roots looking for
/// `pkg` in their `packages` vec, picking the longest-path root on ties.
pub fn root_by_package_query(db: &dyn Db, pkg: Package) -> Option<Root> {
    let mut best: Option<(Root, usize)> = None;
    for &root in db.live_roots() {
        let (LiveRoot::Workspace(r) | LiveRoot::Library(r)) = root else {
            continue;
        };
        if r.packages(db).contains(&pkg) {
            let depth = root_depth(db, r);
            if best.is_none_or(|(_, d)| depth > d) {
                best = Some((r, depth));
            }
        }
    }
    best.map(|(root, _)| root)
}

/// Resolve the [`Package`] that owns `file`, if any.
///
/// Walks live roots' `File -> Package` indexes in workspace-then-library
/// order and returns the first hit. A file belongs to at most one package
/// *entity* (in the nested-root case both roots' `packages` hold the same
/// entity), so first-hit is unambiguous. The orphan bucket has no packages
/// and contributes nothing; stale entities are invisible by design, which
/// is what makes an evicted file's package association clear to `None`.
///
/// Backs [`crate::File::package`], which is the derived replacement for the
/// old `File.package` back-pointer field: the container vecs are now the
/// single source of truth for file ownership.
pub fn package_by_file_query(db: &dyn Db, file: File) -> Option<Package> {
    for &root in db.live_roots() {
        if let LiveRoot::Workspace(r) | LiveRoot::Library(r) = root {
            if let Some(&pkg) = root_file_package_index(db, r).get(&file) {
                return Some(pkg);
            }
        }
    }
    None
}

/// Number of path segments in a root's URL. Used as the tiebreaker by
/// [`root_by_package_query`] when nested roots both claim the same package.
///
/// Counts URL segments directly rather than going through `to_file_path()`.
/// `to_file_path()` errors on Windows for non-OS-style URLs (no drive
/// letter), which would silently collapse all depths to zero and degrade
/// the tiebreaker into "first found wins". Depth is a structural property
/// of the URL hierarchy, so the URL itself is the right source.
fn root_depth(db: &dyn Db, root: Root) -> usize {
    root.path(db)
        .as_url()
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).count())
        .unwrap_or(0)
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

/// Per-root File -> owning Package index. Built from `root.packages` and
/// each package's `files`. Same per-root granularity as [`root_url_index`]:
/// adding or removing a file in this root invalidates only this entry.
/// Backs [`package_by_file_query`].
#[salsa::tracked(returns(ref))]
fn root_file_package_index(db: &dyn Db, root: Root) -> FxHashMap<File, Package> {
    let mut map = FxHashMap::default();
    for &pkg in root.packages(db) {
        for &file in pkg.files(db) {
            map.insert(file, pkg);
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
