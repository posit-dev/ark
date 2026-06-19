//! Library scanner. Drives `LibraryRoots` from `.libPaths()` entries.
//!
//! [`set_library_paths`] is declarative: it reconciles the live set of
//! library roots to exactly the paths it's given. Unchanged paths are
//! left alone (no fs walk, no salsa churn). Removed paths' entities go
//! to [`oak_db::StaleRoot`] so re-adding the same path later reuses the
//! same `File` and `Package` entities (Salsa doesn't GC inputs).
//!
//! Installed packages don't have a watcher today: the LSP currently
//! calls this once at init. The diff path is here ahead of need so
//! libraries and workspaces share the same eviction story.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;
use walkdir::WalkDir;

use crate::inputs::RootExt;
use crate::packages::file_revision;

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
    db.library_roots().set_roots(db).to(new_roots);
}

/// Initial scan of a path that wasn't previously a library root. Walks only the
/// package directories, not the package directory contents. Calls `set_package()`
/// per package directory, returns the freshly-built `Root`.
fn scan_new_library_path<DB: Db + DbInputs>(db: &mut DB, scan_path: &Path, path: FilePath) -> Root {
    let root = Root::new(db, path, RootKind::Library, Vec::new(), Vec::new());

    let mut packages: Vec<Package> = Vec::new();
    for entry in WalkDir::new(scan_path).max_depth(1).min_depth(1) {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_dir() {
            continue;
        }
        let Some(pkg) = register_installed_package(db, root, entry.path()) else {
            continue;
        };
        packages.push(pkg);
    }

    root.set_packages(db).to(packages);
    root
}

/// Register one installed package under `root` without reading any of its
/// files.
///
/// R installs packages at `<libpath>/<PkgName>/`, so the directory basename
/// is the package name. That lets us skip reading `DESCRIPTION` here: we only
/// confirm it exists (that's what marks the directory as a package) and stat
/// `DESCRIPTION` / `NAMESPACE` for the revisions that drive the lazy metadata
/// queries. Version, collation, and namespace are parsed on demand, only if
/// the package is ever imported. Returns `None` for a directory with no
/// `DESCRIPTION` or a non-UTF8 name.
fn register_installed_package<DB: Db + DbInputs>(
    db: &mut DB,
    root: Root,
    package_dir: &Path,
) -> Option<Package> {
    let description_file = package_dir.join("DESCRIPTION");
    if !description_file.is_file() {
        return None;
    }

    let name = package_dir.file_name()?.to_str()?.to_string();
    let description_revision = file_revision(&description_file);
    let namespace_revision = file_revision(&package_dir.join("NAMESPACE"));
    let description_path = FilePath::from_path_buf(description_file)?;

    Some(root.set_package(
        db,
        description_path,
        name,
        description_revision,
        namespace_revision,
        Vec::new(),
        Vec::new(),
    ))
}
