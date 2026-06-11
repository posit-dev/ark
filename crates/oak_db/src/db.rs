use aether_path::FilePath;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

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
/// (`file_by_path_query` etc.) that walk per-root indices which *are* memoized,
/// so salsa records dep edges through those.
///
/// Each concrete db type provides its own forwarding `impl Db`, which is
/// what lets `db.file_by_path(path)` work on both `&dyn Db` (via the trait
/// method) and concrete db references (via the type's impl).
#[salsa::db]
pub trait Db: DbInputs {
    /// Look up the `File` interned at `path`, if any.
    ///
    /// Walks the per-root URL indices in workspace-then-library order,
    /// then falls back to the orphan bucket. The walk short-circuits
    /// on the first hit, so callers depend only on the index maps
    /// actually visited.
    fn file_by_path(&self, path: &FilePath) -> Option<File>;

    /// Look up the `Package` named `name`, applying the following precedence:
    /// - Workspace packages shadow installed ones
    /// - Installed packages in an earlier root shadow later ones
    ///   (mirroring `.libPaths()`).
    fn package_by_name(&self, name: &str) -> Option<Package>;

    /// Resolve the live `Root` that contains `pkg`, if any.
    ///
    /// Returns `None` when the package is only in [`StaleRoot`] (its live
    /// container was previously evicted).
    ///
    /// **Nested roots.** Two roots can claim the same package when one is
    /// nested inside the other, e.g. the frontend opens both `/proj` and
    /// `/proj/sub-pkg` as workspace folders and both scans walk into
    /// `sub-pkg/DESCRIPTION`. Both scans hand the same `Package` entity to
    /// their respective root's `packages` vec, and both vecs keep listing
    /// it for as long as both folders are open (the outer scan re-walks
    /// into `sub-pkg` every time). The overlap is steady state, not
    /// transient, so this query resolves it by picking the longest-path
    /// root. `root_by_file` applies the same rule at the file level.
    fn root_by_package(&self, pkg: Package) -> Option<Root>;

    /// All live roots in lookup-precedence order: workspace folders first, then
    /// library paths (mirroring R's `.libPaths()`), then the orphan bucket.
    /// Stale roots are not included. Salsa-cached and invalidates when one of
    /// `workspace_roots` / `library_roots` / `orphan_root` changes.
    fn live_roots(&self) -> &[LiveRoot];
}

#[salsa::tracked(returns(ref))]
pub(crate) fn live_roots_query(db: &dyn Db) -> Vec<LiveRoot> {
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

/// All files known to the database, in stable order (workspace, library, orphan).
///
/// Used as the workspace-wide candidate pool for find-references: callers
/// apply a textual name filter before building indexes.
///
/// Nested roots overlap on disk, so the same `File` is reachable from several
/// roots (open `/proj` and `/proj/sub-pkg` and the outer scan walks into
/// `sub-pkg`). A `seen` set drops the repeats. Unlike `root_by_package` /
/// `root_by_file`, this query exposes no ownership, just a flat set, so it
/// doesn't matter which root a duplicate is attributed to. We keep the first
/// occurrence, which preserves the traversal order above.
#[salsa::tracked(returns(ref))]
pub fn all_files(db: &dyn Db) -> Vec<File> {
    let mut seen = FxHashSet::default();
    let mut files = Vec::new();

    for &root in db.live_roots() {
        match root {
            LiveRoot::Workspace(r) | LiveRoot::Library(r) => {
                let root_files = r.scripts(db).iter().chain(
                    r.packages(db)
                        .iter()
                        .flat_map(|&pkg| pkg.files(db).iter().chain(pkg.scripts(db))),
                );
                for &file in root_files {
                    if seen.insert(file) {
                        files.push(file);
                    }
                }
            },
            LiveRoot::Orphan(orphan) => {
                for &file in orphan.files(db) {
                    if seen.insert(file) {
                        files.push(file);
                    }
                }
            },
        }
    }

    files
}

/// Files eligible for the workspace symbol index: workspace-root scripts and
/// package files, plus orphan editor buffers. Library roots are excluded, so
/// installed package symbols don't leak into completions or workspace symbols.
/// Mirrors [`all_files`] but skips `LiveRoot::Library`.
#[salsa::tracked(returns(ref))]
pub fn workspace_files(db: &dyn Db) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();

    for &root in db.live_roots() {
        match root {
            LiveRoot::Workspace(r) => {
                let owned = |f: File| root_by_file(db, f) == Some(r);
                files.extend(r.scripts(db).iter().copied().filter(|&f| owned(f)));
                for &pkg in r.packages(db) {
                    let pkg_files = pkg.files(db).iter().chain(pkg.scripts(db));
                    files.extend(pkg_files.copied().filter(|&f| owned(f)));
                }
            },
            LiveRoot::Library(_) => {},
            LiveRoot::Orphan(orphan) => {
                files.extend(orphan.files(db).iter().copied());
            },
        }
    }

    files
}

/// Implementation of [`Db::file_by_path`]. Walks the per-root indices.
///
/// Not itself salsa-tracked (its `&FilePath` argument isn't a salsa
/// entity), but every step is: each [`root_path_index`] call returns a
/// cached map, so adding a file to one root invalidates only that
/// root's index.
pub(crate) fn file_by_path_query(db: &dyn Db, path: &FilePath) -> Option<File> {
    for &root in db.live_roots() {
        let hit = match root {
            LiveRoot::Workspace(r) | LiveRoot::Library(r) => {
                root_path_index(db, r).get(path).copied()
            },
            LiveRoot::Orphan(_) => orphan_path_index(db).get(path).copied(),
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

/// Implementation of [`Db::package_by_name`]. Same shape as
/// [`file_by_path_query`]; orphan has no packages, so it contributes
/// nothing to the walk.
pub(crate) fn package_by_name_query(db: &dyn Db, name: &str) -> Option<Package> {
    for &root in db.live_roots() {
        if let LiveRoot::Workspace(r) | LiveRoot::Library(r) = root {
            if let Some(&pkg) = root_package_index(db, r).get(name) {
                return Some(pkg);
            }
        }
    }
    None
}

/// Implementation of [`Db::root_by_package`]. Walks all live roots looking for
/// `pkg` in their `packages` vec, picking the longest-path root on ties.
pub(crate) fn root_by_package_query(db: &dyn Db, pkg: Package) -> Option<Root> {
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

/// The live root that owns `file`: among the workspace and library roots
/// whose scanned set reaches `file`, the one with the longest path.
///
/// The file-level analogue of [`root_by_package`], used by [`File::root`].
/// Nested roots overlap on disk, so a file reachable from several roots is
/// owned by the deepest one, matching the package tiebreak. Ownership is keyed
/// on reachability (the file is in the root's [`root_path_index`]), not bare
/// path prefix, so the owner always actually contains the file. A bare path
/// prefix would name a freshly-added but not-yet-scanned nested root that
/// doesn't contain the file yet.
///
/// Returns `None` for orphan files (they live in no workspace or library
/// root). [`File::root`] handles that case with a path-prefix fallback.
pub(crate) fn root_by_file(db: &dyn Db, file: File) -> Option<Root> {
    let mut best: Option<(Root, usize)> = None;

    let path = file.path(db);
    for &root in db.live_roots() {
        let (LiveRoot::Workspace(r) | LiveRoot::Library(r)) = root else {
            continue;
        };
        if root_path_index(db, r).contains_key(path) {
            let depth = root_depth(db, r);
            if best.is_none_or(|(_, d)| depth > d) {
                best = Some((r, depth));
            }
        }
    }

    best.map(|(root, _)| root)
}

/// Number of path segments in a root's URL. Used as the tiebreaker by
/// [`root_by_package_query`] and [`root_by_file`] when nested roots both
/// claim the same package or file.
///
/// Counts URL segments directly rather than going through `to_file_path()`.
/// `to_file_path()` errors on Windows for non-OS-style URLs (no drive
/// letter), which would silently collapse all depths to zero and degrade
/// the tiebreaker into "first found wins". Depth is a structural property
/// of the URL hierarchy, so the URL itself is the right source.
fn root_depth(db: &dyn Db, root: Root) -> usize {
    root.path(db)
        .to_url()
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).count())
        .unwrap_or(0)
}

/// Per-root URL -> File index. Salsa caches one map per `Root`;
/// reads `root.scripts`, `root.packages`, each `pkg.files`, and each
/// `pkg.scripts` reachable from this root. Adding or removing a file
/// in *this* root invalidates this entry; other roots stay cached.
#[salsa::tracked(returns(ref))]
fn root_path_index(db: &dyn Db, root: Root) -> FxHashMap<FilePath, File> {
    let mut map = FxHashMap::default();
    for &file in root.scripts(db) {
        map.insert(file.path(db).clone(), file);
    }
    for &pkg in root.packages(db) {
        for &file in pkg.files(db) {
            map.insert(file.path(db).clone(), file);
        }
        for &file in pkg.scripts(db) {
            map.insert(file.path(db).clone(), file);
        }
    }
    map
}

/// Orphan URL -> File index. Reads only `orphan_root().files`.
#[salsa::tracked(returns(ref))]
fn orphan_path_index(db: &dyn Db) -> FxHashMap<FilePath, File> {
    let mut map = FxHashMap::default();
    for &file in db.orphan_root().files(db) {
        map.insert(file.path(db).clone(), file);
    }
    map
}

/// Per-root name -> Package index. Same granularity as
/// [`root_path_index`].
#[salsa::tracked(returns(ref))]
fn root_package_index(db: &dyn Db, root: Root) -> FxHashMap<String, Package> {
    let mut map = FxHashMap::default();
    for &pkg in root.packages(db) {
        map.insert(pkg.name(db).clone(), pkg);
    }
    map
}
