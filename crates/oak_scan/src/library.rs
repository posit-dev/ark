//! Library scanner. Drives `LibraryRoots` from `.libPaths()` entries.
//!
//! [`set_library_paths()`] reconciles live library roots to the supplied
//! paths. Unchanged paths require no filesystem walk or Salsa writes.
//! Removed roots are evicted, but [`oak_db::IdentityMap`] retains their
//! `File` and `Package` identities for reuse if their paths are added again.
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

use crate::eviction::clear_package_sources;
use crate::inputs::replace_package_membership;
use crate::inputs::upsert_package_metadata;
use crate::inputs::RootExt;
use crate::packages::file_revision;
use crate::packages::is_installed_package;

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

    for (old_path, &old_root) in &old {
        if !new_paths.contains(old_path) {
            old_root.evict(db);
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
    set_installed_packages(db, root, list_library(scan_path));
    root
}

/// List installed packages without database access so rescans can run off the
/// main loop. Only direct package directories are visited, not their contents.
///
/// R uses the package name as the directory basename, so no file contents need
/// reading. A directory qualifies when it is an installed package
/// ([`is_installed_package()`]) with a `DESCRIPTION` file. The mtimes of
/// `DESCRIPTION`, `NAMESPACE`, and `INDEX` drive lazy metadata queries, which
/// parse their contents on demand. Install locks, source directories, and
/// directories with non-UTF8 names are skipped.
///
/// The workspace walk prunes every directory [`is_installed_package()`]
/// accepts, a superset of what this listing returns, so a library nested in a
/// workspace never shares a [`Package`] with it.
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
    if !is_installed_package(package_dir) || !description_file.is_file() {
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

/// Reconcile `root.packages` with [`list_library()`]'s entries, reusing
/// [`Package`] entities by `DESCRIPTION` path through [`oak_db::IdentityMap`]
/// and writing only changed inputs.
///
/// Preserve `files` and `scripts` fetched for the current `DESCRIPTION`
/// revision ([`Package::sources_revision()`]). The listing cannot reconstruct
/// them, and [`upsert_package()`](crate::inputs::upsert_package) would replace
/// them with empty lists.
///
/// When [`Package::sources_revision()`] is `None` or differs from the
/// current `DESCRIPTION` revision, [`clear_outdated_sources()`] clears source
/// files and scripts but preserves generated files. This discards sources
/// from an older installation or workspace files from a [`Package`] that
/// became installed. The LSP source scheduler requests sources for the
/// current revision.
///
/// Removed packages leave `root.packages` and their sources are cleared with
/// [`clear_package_sources()`]. The cleared files become invisible to
/// analysis, and the caller keeps open buffers visible with
/// [`crate::inputs::keep_open_files_visible()`].
pub(crate) fn set_installed_packages<DB: Db + DbInputs>(
    db: &mut DB,
    root: Root,
    entries: Vec<InstalledPackageEntry>,
) {
    let mut outdated: Vec<Package> = Vec::new();
    let mut packages: Vec<Package> = Vec::with_capacity(entries.len());

    for entry in entries {
        let package = upsert_package_metadata(
            db,
            entry.description_path,
            entry.name,
            entry.description_revision,
            entry.namespace_revision,
            Some(entry.index_revision),
        );

        if package.sources_revision(db) != Some(entry.description_revision) {
            outdated.push(package);
        }
        packages.push(package);
    }

    clear_outdated_sources(db, &outdated);

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

    clear_package_sources(db, &removed);
}

/// Clear source files and scripts and reset [`Package::sources_revision()`],
/// preserving generated files.
///
/// Generated files have a separate writer, so a source revision mismatch does
/// not invalidate them. Until sources are fetched,
/// [`Package::sources_revision()`] is `None` on every listing. Clearing
/// generated files in that case would discard `base`'s primitive stubs on
/// every library rescan.
fn clear_outdated_sources<DB: Db + DbInputs>(db: &mut DB, packages: &[Package]) {
    for &pkg in packages {
        if pkg.sources_revision(db).is_some() {
            pkg.set_sources_revision(db).to(None);
        }

        replace_package_membership(db, pkg, |db| {
            pkg.set_source_files(db, Vec::new());
            if !pkg.scripts(db).is_empty() {
                pkg.set_scripts(db).to(Vec::new());
            }
        });
    }
}
