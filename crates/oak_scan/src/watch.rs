//! Surgical single-file updates from file-watcher events.
//!
//!
//! Dispatch (`DESCRIPTION`, `.Rprofile`, and `.Renviron` rescans vs surgical
//! add/remove, plus mid-scan buffering) lives on [`crate::ScanScheduler`]. This module just exposes
//! [`add_watched_file`] / [`remove_watched_file`] for the scheduler to call
//! after it has decided a single event can apply surgically against the live
//! root.

use aether_path::FilePath;
use camino::Utf8Path;
use oak_db::classify_in_package;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::PackagePlacement;
use oak_db::Root;
use salsa::Setter;

use crate::inputs::remove_from_orphan;
use crate::inputs::remove_from_pkg_files;
use crate::inputs::upsert_root_file;
use crate::inputs::with_cow_filter;
use crate::inputs::with_cow_push;
use crate::inputs::FileEntry;
use crate::packages::file_revision;
use crate::packages::is_r_file;
use crate::packages::read_description_name;

/// Sync a watched R file with disk and all containing workspace roots.
///
/// New files use the scanner's placement rules within each root, except that
/// `Collate` is not consulted. A new file directly under a package's `R/`
/// directory enters `pkg.files` even if `Collate` omits it. A rescan applies
/// the `Collate` restriction.
///
/// Existing files keep their entity, package placement, and editor override.
/// Their disk revision is refreshed, and missing `root.scripts` memberships
/// are added. Nested roots share file entities but can have different scan
/// results, so an existing entity does not imply membership in every root.
pub(crate) fn add_watched_file<DB: Db + DbInputs>(db: &mut DB, path: FilePath) {
    let Some(fs_path) = path.as_path() else {
        log::warn!("Skipping add_watched_file: URL is not a file path");
        return;
    };

    let revision = file_revision(fs_path.as_std_path());

    // A file can belong to a package in an outer root and to `root.scripts`
    // in an inner root, where the package directory lies outside the root.
    let placements: Vec<Placement> = workspace_roots_containing(db, fs_path)
        .into_iter()
        .filter_map(|root| classify(db, root, fs_path))
        .collect();

    if let Some(existing) = db.file_by_path(&path) {
        // Duplicate watcher events can report the same mtime. Avoid invalidating
        // `source_text()` when the disk revision has not changed.
        if existing.revision(db) != revision {
            existing.set_revision(db).to(revision);
        }
        // Preserve the scanner's package placement. `classify()` ignores
        // `Collate`, so applying its package placement could also add a file
        // omitted by `Collate` to `pkg.files` when it is already in `pkg.scripts`.
        // Only `root.scripts` memberships need repair because nested roots share
        // `Package` entities.
        for placement in placements {
            if let Placement::Script(_) = placement {
                append_to_container(db, existing, placement);
            }
        }
        return;
    }

    if placements.is_empty() {
        // The URL isn't an R file, falls outside every workspace, or lives
        // in a package subdir we don't track (e.g. nested under `R/`).
        return;
    }

    // `File` has a single package backpointer. Point it at the package whose
    // container lists the file, if any, so `remove_watched_file()` finds it.
    let package = placements
        .iter()
        .find_map(|placement| placement.package_backpointer());
    let entry = FileEntry { path, revision };
    let file = upsert_root_file(db, package, entry);
    for placement in placements {
        append_to_container(db, file, placement);
    }
}

/// Append `file` to whichever container `placement` points at, if it
/// isn't already listed. The watcher's single-file equivalent of the
/// scanner's bulk `pkg.set_source_files()` / `root.set_scripts()` atomic replace.
fn append_to_container<DB: Db + DbInputs>(db: &mut DB, file: File, placement: Placement) {
    match placement {
        Placement::Script(root) => {
            if let Some(scripts) = with_cow_push(root.scripts(db), file) {
                root.set_scripts(db).to(scripts);
            }
        },
        Placement::PackageFile(pkg) => {
            pkg.add_source_file(db, file);
        },
        Placement::PackageScript(pkg) => {
            if let Some(scripts) = with_cow_push(pkg.scripts(db), file) {
                pkg.set_scripts(db).to(scripts);
            }
        },
    }
}

/// Unlink a deleted file so [`oak_db::Db::file_by_path()`] no longer finds it.
///
/// Package removal relies on the file's package backpointer. Other packages
/// are not searched. Root script memberships and orphan membership are removed
/// independently because the same file can appear in multiple roots.
///
/// Salsa does not support deleting inputs, so the [`File`] entity remains in
/// the graph even after its container references are removed.
pub(crate) fn remove_watched_file<DB: Db + DbInputs>(db: &mut DB, path: FilePath) {
    let Some(file) = db.file_by_path(&path) else {
        return;
    };

    if let Some(pkg) = file.package(db) {
        remove_from_pkg_files(db, pkg, file);
    }
    for &root in &db.workspace_roots().roots(db).clone() {
        if let Some(scripts) = with_cow_filter(root.scripts(db), file) {
            root.set_scripts(db).to(scripts);
        }
    }
    remove_from_orphan(db, file);
}

#[derive(Copy, Clone)]
enum Placement {
    Script(Root),
    PackageFile(Package),
    PackageScript(Package),
}

impl Placement {
    fn package_backpointer(self) -> Option<Package> {
        match self {
            Placement::Script(_) => None,
            Placement::PackageFile(pkg) | Placement::PackageScript(pkg) => Some(pkg),
        }
    }
}

/// Classify a file path against workspace root `root`, which contains it.
///
/// Returns the placement, or `None` if the file isn't an R file or sits
/// in a package subdir we don't track (e.g. `<pkg>/R/subdir/` nested
/// below the flat namespace).
fn classify<DB: Db + DbInputs>(db: &DB, root: Root, path: &Utf8Path) -> Option<Placement> {
    if !is_r_file(path.as_std_path()) {
        return None;
    }

    let root_path = root.path(db).as_path()?;

    // Find the nearest ancestor (within `root_path`) that contains a
    // `DESCRIPTION`. None means no package above the file, so it's a
    // top-level workspace script.
    let pkg_dir = path
        .ancestors()
        .skip(1)
        .take_while(|ancestor| ancestor.starts_with(root_path))
        .find(|ancestor| ancestor.join("DESCRIPTION").is_file());

    let Some(pkg_dir) = pkg_dir else {
        return Some(Placement::Script(root));
    };

    let pkg_name = read_description_name(pkg_dir.as_std_path())?;
    let pkg = root
        .packages(db)
        .iter()
        .find(|pkg| pkg.name(db) == &pkg_name)
        .copied()?;

    match classify_in_package(pkg_dir.as_std_path(), path.as_std_path()) {
        PackagePlacement::File => Some(Placement::PackageFile(pkg)),
        PackagePlacement::Script => Some(Placement::PackageScript(pkg)),
        PackagePlacement::Skip => None,
    }
}

fn workspace_roots_containing<DB: Db + DbInputs>(db: &DB, path: &Utf8Path) -> Vec<Root> {
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter(|root| match root.path(db).as_path() {
            Some(root_path) => path.starts_with(root_path),
            None => false,
        })
        .copied()
        .collect()
}
