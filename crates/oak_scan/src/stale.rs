//! Eviction logic shared by the library and workspace `set_*_paths` helpers.
//!
//! When a `set_*_paths` call drops a `Root`, its `File` and `Package` entities
//! are still alive in salsa storage (no GC). We rehome them based on whether
//! they're editor-owned:
//!
//! - **Editor-owned files** -> `OrphanRoot`. The user has a buffer open;
//!   the file should keep showing up in analysis even though the
//!   workspace folder went away. The buffer is the source of truth for
//!   its contents until `didClose`.
//!
//!   `OrphanRoot` has no `packages` field, so an evicted package file
//!   loses its package association: `file.package` clears to `None` and
//!   analysis treats it as a standalone script for as long as the
//!   workspace is removed. If the workspace comes back, `upsert_root_file`
//!   finds the same `File` via `OrphanRoot` and re-promotes it into
//!   `pkg.files`, restoring the package context.
//!
//! - **Everything else** -> `StaleRoot`. Invisible to analysis, available
//!   for entity reuse on the next `set_*_paths` that brings the path
//!   back (can happen e.g. in multi-repo workflows where a workspace path is
//!   added and removed).
//!
//! `Package` entities always go to stale: there's no editor-owned analogue for
//! packages.

use std::collections::HashSet;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use rustc_hash::FxHashMap;
use salsa::Setter;

use crate::inputs::with_cow_filter;
use crate::inputs::with_cow_remove;

/// Drop `root` from its live container, rehoming files and packages to
/// `OrphanRoot` / `StaleRoot` as described in the module doc.
///
/// `editor_owned` is `None` for callers without an editor concept (the library
/// scanner) and `Some(&set)` for the workspace scanner. Files in `editor_owned`
/// go to `OrphanRoot`. The rest go to `StaleRoot`.
///
/// Doesn't touch `LibraryRoots` / `WorkspaceRoots`. The caller is responsible
/// for rebuilding those Vec inputs with `root` excluded.
///
/// Internal implementation: the public API is [`crate::RootExt::set_stale`].
pub(crate) fn set_root_stale<DB: Db + DbInputs>(
    db: &mut DB,
    root: Root,
    editor_owned: Option<&HashSet<FilePath>>,
) {
    let packages: Vec<Package> = root.packages(db).clone();

    let mut all_files: Vec<File> = root.scripts(db).to_vec();
    for &pkg in &packages {
        all_files.extend(pkg.files(db).iter().copied());
        all_files.extend(pkg.scripts(db).iter().copied());
    }

    // Clear `file.package` first: by the time these files land in their new
    // home, their old `Package` entity is itself in stale and the backpointer
    // would lie about live containment. Setting to `None` here keeps the
    // placement invariant honest.
    for &file in &all_files {
        file.set_package(db).to(None);
    }

    let (to_orphan, to_stale): (Vec<File>, Vec<File>) = match editor_owned {
        Some(owned) => all_files
            .into_iter()
            .partition(|f| owned.contains(f.path(db))),
        None => (Vec::new(), all_files),
    };

    if !to_orphan.is_empty() {
        let orphan = db.orphan_root();
        let mut files = orphan.files(db).clone();
        files.extend(to_orphan);
        orphan.set_files(db).to(files);
    }

    let stale = db.stale_root();
    if !to_stale.is_empty() {
        let mut files = stale.files(db).clone();
        files.extend(to_stale);
        stale.set_files(db).to(files);
    }

    if !packages.is_empty() {
        let mut stale_packages = stale.packages(db).clone();
        for pkg in &packages {
            if !stale_packages.contains(pkg) {
                stale_packages.push(*pkg);
            }
        }
        stale.set_packages(db).to(stale_packages);
    }

    // Clear the dropped root's containers and each package's files / scripts
    // vec. The packages themselves now live in `stale_root.packages`. Keeping
    // their `files` populated would leave stale references that
    // `package_by_path` can resurrect with inconsistent contents.
    root.set_scripts(db).to(Vec::new());
    for &pkg in &packages {
        pkg.set_files(db).to(Vec::new());
        pkg.set_scripts(db).to(Vec::new());
    }
    root.set_packages(db).to(Vec::new());
}

pub(crate) fn remove_from_stale_files<DB: Db + DbInputs>(db: &mut DB, file: File) {
    let stale = db.stale_root();
    if let Some(files) = with_cow_remove(stale.files(db), file) {
        stale.set_files(db).to(files);
    }
}

pub(crate) fn remove_from_stale_packages<DB: Db + DbInputs>(db: &mut DB, pkg: Package) {
    let stale = db.stale_root();
    if let Some(packages) = with_cow_filter(stale.packages(db), pkg) {
        stale.set_packages(db).to(packages);
    }
}

/// Look up a stale `File` by URL. The scanner's upsert helpers call this to
/// fall back to the eviction bucket after `oak_db::Db::file_by_path` misses,
/// reusing the evicted entity instead of minting a new one.
pub(crate) fn stale_file_by_path(db: &dyn Db, path: &FilePath) -> Option<File> {
    stale_path_index(db).get(path).copied()
}

/// Stale file URL -> File index. Reads only `stale_root().files`. Analysis is
/// stale-blind by design (`oak_db::Db::file_by_path` never consults this), so
/// the scanner is the only reader, via [`stale_file_by_path`] when re-adding a
/// path.
#[salsa::tracked(returns(ref))]
fn stale_path_index(db: &dyn Db) -> FxHashMap<FilePath, File> {
    let mut map = FxHashMap::default();
    for &file in db.stale_root().files(db) {
        map.insert(file.path(db).clone(), file);
    }
    map
}

/// Stale DESCRIPTION URL -> Package index. The eviction-bucket counterpart to
/// the live per-root `root_package_path_index`; consulted by `package_by_path`
/// as its stale fallback.
#[salsa::tracked(returns(ref))]
pub(crate) fn stale_package_path_index(db: &dyn Db) -> FxHashMap<FilePath, Package> {
    let mut map = FxHashMap::default();
    for &pkg in db.stale_root().packages(db) {
        map.insert(pkg.description_path(db).clone(), pkg);
    }
    map
}
