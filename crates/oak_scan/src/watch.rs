//! Surgical single-file updates from file-watcher events.
//!
//!
//! Dispatch (DESCRIPTION rescan vs surgical add/remove, plus mid-scan
//! buffering) lives on [`crate::ScanScheduler`]. This module just exposes
//! [`add_watched_file`] / [`remove_watched_file`] for the scheduler to call
//! after it has decided a single event can apply surgically against the live
//! root.

use aether_path::FilePath;
use camino::Utf8Path;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use salsa::Setter;

use crate::inputs::remove_from_orphan;
use crate::inputs::remove_from_pkg_files;
use crate::inputs::upsert_root_file;
use crate::inputs::with_cow_filter;
use crate::inputs::with_cow_push;
use crate::inputs::FileEntry;
use crate::packages::classify_in_package;
use crate::packages::file_revision;
use crate::packages::is_r_file;
use crate::packages::read_description_name;
use crate::packages::PackagePlacement;

/// Driver-neutral file event. Drivers (the LSP, tests, ...) translate
/// their native event type into this shape.
#[derive(Clone, Debug)]
pub struct FileEvent {
    pub kind: FileEventKind,
    pub path: FilePath,
}

/// Mirrors the three states an OS file watcher reports.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FileEventKind {
    Created,
    Changed,
    Deleted,
}

/// React to a Created or Changed event on an R file. Idempotent: if a `File`
/// already exists at this URL, its contents are updated and its placement is
/// left alone. If not, the URL is classified against the current workspace
/// roots and the new file lands in the right container: `pkg.files` for
/// `<pkg>/R/*.R`, `pkg.scripts` for other R files under a package
/// (tests/, inst/, vignettes/, ...), `root.scripts` for R files outside
/// every package. Mirrors the placement the bulk scanner would pick.
pub(crate) fn add_watched_file<DB: Db + DbInputs>(db: &mut DB, path: FilePath) {
    let Some(fs_path) = path.as_path() else {
        log::warn!("Skipping add_watched_file: URL is not a file path");
        return;
    };

    let revision = file_revision(fs_path.as_std_path());

    if let Some(existing) = db.file_by_path(&path) {
        // Bump the revision and leave any editor override in place: if the
        // file is open in the editor, the override still wins in
        // `source_text`, so the on-disk change is ignored until the buffer
        // closes. Otherwise the bump forces the next `source_text` to re-read.
        existing.set_revision(db).to(revision);
        return;
    }

    let Some(placement) = classify(db, fs_path) else {
        // Either the URL falls outside every workspace, or it lives
        // inside a package subdir we don't track (tests/, inst/, ...).
        return;
    };

    let entry = FileEntry { path, revision };
    let file = upsert_root_file(db, placement.package_backpointer(), entry);
    append_to_container(db, file, placement);
}

/// Append `file` to whichever container `placement` points at, if it
/// isn't already listed. The watcher's single-file equivalent of the
/// scanner's bulk `pkg.set_files()` / `root.set_scripts()` atomic replace.
fn append_to_container<DB: Db + DbInputs>(db: &mut DB, file: File, placement: Placement) {
    match placement {
        Placement::Script(root) => {
            if let Some(scripts) = with_cow_push(root.scripts(db), file) {
                root.set_scripts(db).to(scripts);
            }
        },
        Placement::PackageFile(pkg) => {
            if let Some(files) = with_cow_push(pkg.files(db), file) {
                pkg.set_files(db).to(files);
            }
        },
        Placement::PackageScript(pkg) => {
            if let Some(scripts) = with_cow_push(pkg.scripts(db), file) {
                pkg.set_scripts(db).to(scripts);
            }
        },
    }
}

/// React to a Deleted event. Unlinks the file from whichever container
/// holds it so [`oak_db::Db::file_by_path`] stops returning it. The
/// `File` entity itself stays in the salsa graph (salsa doesn't
/// support deleting inputs), but with no container references nothing
/// will reach it.
pub(crate) fn remove_watched_file<DB: Db + DbInputs>(db: &mut DB, path: FilePath) {
    let Some(file) = db.file_by_path(&path) else {
        return;
    };

    if let Some(pkg) = file.package(db) {
        remove_from_pkg_files(db, pkg, file);
        return;
    }

    for &root in &db.workspace_roots().roots(db).clone() {
        if let Some(scripts) = with_cow_filter(root.scripts(db), file) {
            root.set_scripts(db).to(scripts);
            return;
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

/// Classify a file path against the current workspace tree.
///
/// Returns the placement, or `None` if the file falls outside every
/// workspace or sits in a package subdir we don't track (e.g.
/// `<pkg>/R/subdir/` nested below the flat namespace).
fn classify<DB: Db + DbInputs>(db: &DB, path: &Utf8Path) -> Option<Placement> {
    if !is_r_file(path.as_std_path()) {
        return None;
    }

    let root = workspace_root_containing(db, path)?;
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

fn workspace_root_containing<DB: Db + DbInputs>(db: &DB, path: &Utf8Path) -> Option<Root> {
    db.workspace_roots()
        .roots(db)
        .iter()
        .find(|root| match root.path(db).as_path() {
            Some(root_path) => path.starts_with(root_path),
            None => false,
        })
        .copied()
}
