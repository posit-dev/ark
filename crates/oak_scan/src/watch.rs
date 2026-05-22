//! Surgical updates from file-watcher events.
//!
//! The workspace scanner ([`crate::workspace`]) is the bulk path: it
//! walks an entire root and rebuilds packages and scripts. The helpers
//! here handle one file event at a time, so a burst of file watcher
//! notifications doesn't trigger a full rescan per event.
//!
//! [`apply_watcher_events`] is the entry point used by drivers (the LSP,
//! tests, eventually anything else that gets a stream of file events).
//! Drivers translate their native event type into [`FileEvent`] and
//! call in, and `apply_watcher_events` does the routing:
//!
//! - DESCRIPTION events fall back to
//!   [`crate::workspace::rescan_workspace_root`] on the containing
//!   workspace root, deduped within the batch. A `DESCRIPTION` add or
//!   removal can promote or demote a whole directory, which is too
//!   tangled for a one-file update.
//!
//! - R file events route through [`add_watched_file`] (Created / Changed) or
//!   [`remove_watched_file`] (Deleted). The `skip` set lets the driver hold
//!   back URLs whose contents it owns (the LSP uses this for files
//!   the editor has open).

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use salsa::Setter;

use crate::inputs::remove_from_pkg_files;
use crate::inputs::upsert_root_file;
use crate::inputs::FileEntry;
use crate::packages::is_r_file;
use crate::packages::read_description_name;

/// Driver-neutral file event. Drivers (the LSP, tests, ...) translate
/// their native event type into this shape.
#[derive(Clone, Debug)]
pub struct FileEvent {
    pub kind: FileEventKind,
    pub url: UrlId,
}

/// Mirrors the three states an OS file watcher reports.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FileEventKind {
    Created,
    Changed,
    Deleted,
}

/// Apply a batch of file events to the oak input tree.
///
/// DESCRIPTION events are deduped to one rescan per containing root.
/// R-file events route through [`add_watched_file`] / [`remove_watched_file`]. URLs
/// in `skip` are not touched even if their event is for an R file;
/// callers use this to defer to an in-memory source of truth (e.g.
/// the LSP's editor buffers).
pub(crate) fn apply_watcher_events<DB: Db + DbInputs>(
    db: &mut DB,
    events: Vec<FileEvent>,
    skip: &HashSet<UrlId>,
) {
    let roots = workspace_root_paths(db);
    let mut stale_roots: HashSet<Root> = HashSet::new();

    for event in events {
        let Ok(path) = event.url.to_file_path() else {
            continue;
        };

        if path.file_name().is_some_and(|n| n == "DESCRIPTION") {
            if let Some(root) = roots
                .iter()
                .find(|(p, _)| path.starts_with(p))
                .map(|(_, r)| *r)
            {
                stale_roots.insert(root);
            }
            continue;
        }

        if skip.contains(&event.url) {
            continue;
        }

        match event.kind {
            FileEventKind::Created | FileEventKind::Changed => match std::fs::read_to_string(&path)
            {
                Ok(contents) => add_watched_file(db, event.url, contents),
                Err(err) => log::warn!("Skipped watched file {}: {err:?}", path.display()),
            },
            FileEventKind::Deleted => remove_watched_file(db, event.url),
        }
    }

    for root in stale_roots {
        crate::workspace::rescan_workspace_root(db, root);
    }
}

fn workspace_root_paths<DB: Db + DbInputs>(db: &DB) -> Vec<(PathBuf, Root)> {
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter_map(|r| match r.path(db).to_file_path() {
            Ok(p) => Some((p, *r)),
            Err(err) => {
                log::warn!("Skipping workspace root: {err}");
                None
            },
        })
        .collect()
}

/// React to a Created or Changed event on an R file. Idempotent: if a `File`
/// already exists at this URL, its contents are updated and its placement is
/// left alone. If not, the URL is classified against the current workspace
/// roots and the new file lands in the right container: `pkg.files` for
/// `<pkg>/R/*.R`, `pkg.scripts` for other R files under a package
/// (tests/, inst/, vignettes/, ...), `root.scripts` for R files outside
/// every package. Mirrors the placement the bulk scanner would pick.
pub(crate) fn add_watched_file<DB: Db + DbInputs>(db: &mut DB, url: UrlId, contents: String) {
    if let Some(existing) = db.file_by_url(&url) {
        existing.set_contents(db).to(contents);
        return;
    }

    let Ok(path) = url.to_file_path() else {
        log::warn!("Skipping add_watched_file: URL is not a file path");
        return;
    };

    let Some(placement) = classify(db, &path) else {
        // Either the URL falls outside every workspace, or it lives
        // inside a package subdir we don't track (tests/, inst/, ...).
        return;
    };

    let entry = FileEntry { url, contents };
    let file = upsert_root_file(db, placement.package_backpointer(), entry);
    append_to_container(db, file, placement);
}

/// Append `file` to whichever container `placement` points at, if it
/// isn't already listed. The watcher's single-file equivalent of the
/// scanner's bulk `pkg.set_files()` / `root.set_scripts()` atomic replace.
fn append_to_container<DB: Db + DbInputs>(db: &mut DB, file: File, placement: Placement) {
    match placement {
        Placement::Script(root) => {
            let mut scripts = root.scripts(db).clone();
            if !scripts.contains(&file) {
                scripts.push(file);
                root.set_scripts(db).to(scripts);
            }
        },
        Placement::PackageFile(pkg) => {
            let mut files = pkg.files(db).clone();
            if !files.contains(&file) {
                files.push(file);
                pkg.set_files(db).to(files);
            }
        },
        Placement::PackageScript(pkg) => {
            let mut scripts = pkg.scripts(db).clone();
            if !scripts.contains(&file) {
                scripts.push(file);
                pkg.set_scripts(db).to(scripts);
            }
        },
    }
}

/// React to a Deleted event. Unlinks the file from whichever container
/// holds it so [`oak_db::Db::file_by_url`] stops returning it. The
/// `File` entity itself stays in the salsa graph (salsa doesn't
/// support deleting inputs), but with no container references nothing
/// will reach it.
pub(crate) fn remove_watched_file<DB: Db + DbInputs>(db: &mut DB, url: UrlId) {
    let Some(file) = db.file_by_url(&url) else {
        return;
    };

    if let Some(pkg) = file.package(db) {
        remove_from_pkg_files(db, pkg, file);
        return;
    }

    for &root in &db.workspace_roots().roots(db).clone() {
        let mut scripts = root.scripts(db).clone();
        if scripts.contains(&file) {
            scripts.retain(|f| *f != file);
            root.set_scripts(db).to(scripts);
            return;
        }
    }

    let orphan = db.orphan_root();
    let mut files = orphan.files(db).clone();
    if files.contains(&file) {
        files.retain(|f| *f != file);
        orphan.set_files(db).to(files);
    }
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
fn classify<DB: Db + DbInputs>(db: &DB, path: &Path) -> Option<Placement> {
    if !is_r_file(path) {
        return None;
    }

    let root = workspace_root_containing(db, path)?;
    let root_path = root.path(db).to_file_path().ok()?;

    // Find the nearest ancestor (within `root_path`) that contains a
    // `DESCRIPTION`. None means no package above the file, so it's a
    // top-level workspace script.
    let pkg_dir = path
        .ancestors()
        .skip(1)
        .take_while(|p| p.starts_with(&root_path))
        .find(|p| p.join("DESCRIPTION").is_file());

    let Some(pkg_dir) = pkg_dir else {
        return Some(Placement::Script(root));
    };

    let pkg_name = read_description_name(pkg_dir)?;
    let pkg = root
        .packages(db)
        .iter()
        .find(|p| p.name(db) == &pkg_name)
        .copied()?;

    let r_dir = pkg_dir.join("R");
    if path.parent() == Some(&r_dir) {
        return Some(Placement::PackageFile(pkg));
    }
    if path.starts_with(&r_dir) {
        // Nested below `<pkg>/R/`. The bulk scanner's `scan_r_files`
        // reads only direct children of `R/`, and `scan_package_scripts`
        // excludes anything under `R/`. The watcher matches that.
        return None;
    }
    Some(Placement::PackageScript(pkg))
}

fn workspace_root_containing<DB: Db + DbInputs>(db: &DB, path: &Path) -> Option<Root> {
    db.workspace_roots()
        .roots(db)
        .iter()
        .find(|r| match r.path(db).to_file_path() {
            Ok(p) => path.starts_with(&p),
            Err(_) => false,
        })
        .copied()
}
