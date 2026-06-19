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

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;
use walkdir::WalkDir;

use crate::inputs::RootExt;
use crate::packages::read_package_metadata;

/// Reconcile `LibraryRoots` to exactly `paths`. Called through
/// [`crate::DbScan::set_library_paths`]. Order in `LibraryRoots.roots`
/// follows `paths`, matching R's `.libPaths()` precedence.
pub(crate) fn set_library_paths<DB: Db + DbInputs>(db: &mut DB, paths: &[PathBuf]) {
    let new: Vec<(PathBuf, UrlId)> = paths
        .iter()
        .filter_map(|p| {
            let url = UrlId::from_file_path(p).ok()?;
            Some((p.clone(), url))
        })
        .collect();
    let new_urls: HashSet<UrlId> = new.iter().map(|(_, u)| u.clone()).collect();

    let old: HashMap<UrlId, Root> = db
        .library_roots()
        .roots(db)
        .iter()
        .map(|r| (r.path(db).clone(), *r))
        .collect();

    // Evict roots not in the new set. Since library files aren't editor-owned,
    // we pass `None` so everything routes to stale.
    for (old_url, &old_root) in &old {
        if !new_urls.contains(old_url) {
            old_root.set_stale(db, None);
        }
    }

    // Build the new roots list in order: reuse the existing `Root` for
    // unchanged paths (no rescan, that's handled by the watcher), scan the rest.
    let mut new_roots = Vec::with_capacity(new.len());
    for (path, url) in new {
        let root = match old.get(&url) {
            Some(&r) => r,
            None => scan_new_library_path(db, &path, url),
        };
        new_roots.push(root);
    }
    db.library_roots().set_roots(db).to(new_roots);
}

/// Initial scan of a path that wasn't previously a library root. Walks only the
/// package directories, not the package directory contents. Calls `set_package()`
/// per package directory, returns the freshly-built `Root`.
fn scan_new_library_path<DB: Db + DbInputs>(db: &mut DB, path: &Path, url: UrlId) -> Root {
    let root = Root::new(db, url, RootKind::Library, Vec::new(), Vec::new());

    let mut packages: Vec<Package> = Vec::new();
    for entry in WalkDir::new(path).max_depth(1).min_depth(1) {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_dir() {
            continue;
        }
        let Some(pkg) = read_package_metadata(entry.path()) else {
            continue;
        };
        packages.push(root.set_package(
            db,
            pkg.description_url,
            pkg.name,
            pkg.version,
            pkg.namespace,
            pkg.files,
            pkg.scripts,
            pkg.collation,
        ));
    }

    root.set_packages(db).to(packages);
    root
}
