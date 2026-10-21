//! Library scanner. Drives `LibraryRoots` from `.libPaths()` entries.
//!
//! [`set_library_paths`] is declarative: it reconciles the live set of
//! library roots to exactly the paths it's given. Unchanged paths are
//! left alone (no fs walk, no salsa churn). Removed paths' entities go
//! to [`oak_db::StaleRoot`] so re-adding the same path later reuses the
//! same `File` and `Package` entities (Salsa doesn't GC inputs).
//!
//! The LSP watches direct library entries, and [`crate::ScanScheduler`] rescans
//! the affected library. Changes inside an existing package directory are not
//! watched. Initial scans and rescans share [`list_library()`] and
//! [`set_installed_packages()`] to keep listing and reconciliation consistent.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::FileRevision;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;
use walkdir::WalkDir;

use crate::inputs::upsert_package;
use crate::inputs::RootExt;
use crate::lookup::package_by_path;
use crate::packages::file_revision;
use crate::stale::rehome_files;

/// Metadata from [`list_library()`] that lets [`set_installed_packages()`]
/// register or refresh a [`Package`] without further filesystem access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstalledPackageEntry {
    pub name: String,
    pub description_path: FilePath,
    pub description_revision: FileRevision,
    pub namespace_revision: FileRevision,
    pub index_revision: FileRevision,
}

/// Reconcile `LibraryRoots` to exactly `paths`. Called through
/// [`crate::DbScan::set_library_paths`]. Order in `LibraryRoots.roots`
/// follows `paths`, matching R's `.libPaths()` precedence.
pub(crate) fn set_library_paths<DB: Db + DbInputs>(db: &mut DB, paths: &[PathBuf]) {
    let new: Vec<(PathBuf, FilePath)> = paths
        .iter()
        .filter_map(|p| {
            let path = FilePath::from_path_buf(p.clone())?;
            Some((p.clone(), path))
        })
        .collect();
    let new_paths: HashSet<FilePath> = new.iter().map(|(_, path)| path.clone()).collect();

    let old: HashMap<FilePath, Root> = db
        .library_roots()
        .roots(db)
        .iter()
        .map(|r| (r.path(db).clone(), *r))
        .collect();

    // Evict roots not in the new set. Since library files aren't editor-owned,
    // we pass `None` so everything routes to stale.
    for (old_path, &old_root) in &old {
        if !new_paths.contains(old_path) {
            old_root.set_stale(db, None);
        }
    }

    // Build the new roots list in order: reuse the existing `Root` for
    // unchanged paths (no rescan, that's handled by the watcher), scan the rest.
    let mut new_roots = Vec::with_capacity(new.len());
    for (scan_path, path) in new {
        let root = match old.get(&path) {
            Some(&r) => r,
            None => scan_new_library_path(db, &scan_path, path),
        };
        new_roots.push(root);
    }
    if db.library_roots().roots(db) != &new_roots {
        db.library_roots().set_roots(db).to(new_roots);
    }
}

fn scan_new_library_path<DB: Db + DbInputs>(db: &mut DB, scan_path: &Path, path: FilePath) -> Root {
    let root = Root::new(
        db,
        path,
        RootKind::Library,
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let editor_owned = None;
    set_installed_packages(db, root, list_library(scan_path), editor_owned);
    root
}

/// List installed packages without database access so rescans can run off the
/// main loop. Only direct package directories are visited, not their contents.
///
/// R uses the package name as the directory basename, so no file contents need
/// reading. A directory qualifies when `DESCRIPTION` is a file. The mtimes of
/// `DESCRIPTION`, `NAMESPACE`, and `INDEX` drive lazy metadata queries, which
/// parse their contents on demand. Directories without a `DESCRIPTION` file
/// (including install locks) or with non-UTF8 names are skipped.
///
/// Stable directory-name order lets [`set_installed_packages()`] skip the
/// `root.packages` write on unchanged rescans. Filesystem iteration order alone
/// is unspecified.
pub(crate) fn list_library(path: &Path) -> Vec<InstalledPackageEntry> {
    // renv libraries link each package directory into renv's global cache
    // (symlinks on Unix, junctions on Windows). Without `follow_links()`,
    // `file_type()` reports the link rather than a directory and the package is
    // skipped. `entry.path()` stays the link path inside the library, matching
    // what R reports for the installed package.
    let walker = WalkDir::new(path)
        .max_depth(1)
        .min_depth(1)
        .follow_links(true)
        .sort_by_file_name();

    let mut packages = Vec::new();
    for entry in walker {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_dir() {
            continue;
        }
        let Some(package) = read_installed_package(entry.path()) else {
            continue;
        };
        packages.push(package);
    }
    packages
}

fn read_installed_package(package_dir: &Path) -> Option<InstalledPackageEntry> {
    let description_file = package_dir.join("DESCRIPTION");
    if !description_file.is_file() {
        return None;
    }

    let name = package_dir.file_name()?.to_str()?.to_string();
    let description_revision = file_revision(&description_file);
    let namespace_revision = file_revision(&package_dir.join("NAMESPACE"));
    let index_revision = file_revision(&package_dir.join("INDEX"));
    let description_path = FilePath::from_path_buf(description_file)?;

    Some(InstalledPackageEntry {
        name,
        description_path,
        description_revision,
        namespace_revision,
        index_revision,
    })
}

/// Reconcile `root.packages` with [`list_library()`]'s entries, reusing live or
/// stale [`Package`] entities by `DESCRIPTION` path and writing only changed inputs.
///
/// Preserve fetched `files` and `scripts` when the `DESCRIPTION` revision is
/// unchanged. The listing cannot reconstruct them, and [`RootExt::set_package()`]
/// would replace them with empty lists.
///
/// A changed `DESCRIPTION` revision rehomes the old sources through
/// [`rehome_files()`], so they no longer belong to the upgraded installation.
/// The revision change also lets the LSP source scheduler request sources again.
/// Packages held by a workspace root keep their files because the workspace
/// scan owns those source lists.
///
/// Removed packages are not moved to [`oak_db::StaleRoot`]. Once no live or
/// stale root contains the old entity, a later reinstall creates a new one,
/// allowing the LSP source scheduler to fetch sources for the new installation.
///
/// Files of removed packages stay in place if another live root holds the
/// package. Otherwise [`rehome_files()`] moves them to `OrphanRoot` when their
/// paths are in `editor_owned`, or to `StaleRoot`. This preserves open buffers
/// for analysis and keeps closed files available to the upsert helpers' stale
/// lookup. Files left in neither container would be duplicated on a later
/// [`crate::DbScan::upsert_editor()`] or [`crate::DbScan::set_package_sources()`]
/// at the same path.
pub(crate) fn set_installed_packages<DB: Db + DbInputs>(
    db: &mut DB,
    root: Root,
    entries: Vec<InstalledPackageEntry>,
    editor_owned: Option<&HashSet<FilePath>>,
) {
    let mut upgraded: Vec<Package> = Vec::new();
    let mut packages: Vec<Package> = Vec::with_capacity(entries.len());

    for entry in entries {
        let previous_revision = package_by_path(db, &entry.description_path)
            .map(|package| package.description_revision(db));

        let package = upsert_package(
            db,
            entry.description_path,
            entry.name,
            entry.description_revision,
            entry.namespace_revision,
            Some(entry.index_revision),
        );

        let revision_changed =
            previous_revision.is_some_and(|revision| revision != entry.description_revision);
        if revision_changed && !is_workspace_package(db, package) {
            upgraded.push(package);
        }
        packages.push(package);
    }

    rehome_files(db, &upgraded, Vec::new(), editor_owned);

    if root.packages(db) == &packages {
        return;
    }

    let removed: Vec<Package> = root
        .packages(db)
        .iter()
        .filter(|package| !packages.contains(package))
        .copied()
        .collect();
    root.set_packages(db).to(packages);

    // A library inside a workspace can share a `Package` with that workspace's
    // scan. Rehome only the files of packages no live root holds anymore.
    let orphaned: Vec<Package> = removed
        .into_iter()
        .filter(|package| db.root_by_package(*package).is_none())
        .collect();
    rehome_files(db, &orphaned, Vec::new(), editor_owned);
}

fn is_workspace_package(db: &dyn Db, package: Package) -> bool {
    db.workspace_roots()
        .roots(db)
        .iter()
        .any(|root| root.packages(db).contains(&package))
}
