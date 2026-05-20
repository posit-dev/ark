//! Library scanner. Drives `LibraryRoots` from `.libPaths()` entries.
//!
//! Installed packages are (currently) considered static for the session:
//! there's no watcher, no incremental updates. A library refresh requires an
//! LSP restart. The scanner runs once, populates `LibraryRoots`, and is done.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;
use walkdir::WalkDir;

use crate::inputs::RootExt;
use crate::packages::read_package;

/// Scan each library path and register the discovered packages under
/// `LibraryRoots`. Called through [`crate::DbExt::scan_library_paths`].
pub(crate) fn scan_library_paths<DB: Db + DbInputs>(db: &mut DB, paths: &[PathBuf]) {
    let existing: HashMap<UrlId, Root> = db
        .library_roots()
        .roots(db)
        .iter()
        .map(|r| (r.path(db).clone(), *r))
        .collect();

    let mut roots = Vec::with_capacity(paths.len());
    for path in paths {
        match scan_library_path(db, path, &existing) {
            Some(root) => roots.push(root),
            None => log::warn!("Skipped library path: {}", path.display()),
        }
    }
    db.library_roots().set_roots(db).to(roots);
}

fn scan_library_path<DB: Db + DbInputs>(
    db: &mut DB,
    path: &Path,
    existing: &HashMap<UrlId, Root>,
) -> Option<Root> {
    let url = UrlId::from_file_path(path).ok()?;
    let root = match existing.get(&url) {
        Some(&r) => r,
        None => Root::new(db, url, RootKind::Library, Vec::new(), Vec::new()),
    };

    // Direct children of `path` are candidate package directories.
    let mut packages: Vec<Package> = Vec::new();
    for entry in WalkDir::new(path).max_depth(1).min_depth(1) {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_dir() {
            continue;
        }
        let Some(pkg) = read_package(entry.path()) else {
            continue;
        };
        packages.push(root.set_package(
            db,
            pkg.name,
            pkg.version,
            pkg.namespace,
            pkg.files,
            pkg.collation,
        ));
    }

    root.set_packages(db).to(packages);
    Some(root)
}
