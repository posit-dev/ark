//! Workspace scanner. Drives `WorkspaceRoots` from the editor's open folders.
//!
//! [`set_workspace_paths`] is declarative: it reconciles the live set of
//! workspace roots to exactly the paths it's given. Unchanged paths are left
//! alone (the watcher handles in-folder changes via
//! [`rescan_workspace_root`]). Removed paths are evicted; their files route
//! to `OrphanRoot` if editor-owned, otherwise to `StaleRoot` for entity reuse
//! on re-add. New paths are scanned: `DESCRIPTION` files at any depth
//! (honouring `.gitignore`), plus top-level R scripts.

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

use crate::inputs::RootExt;
use crate::packages::scan_workspace;

/// Reconcile `WorkspaceRoots` to exactly `paths`. Called through
/// [`crate::DbScan::set_workspace_paths`].
pub(crate) fn set_workspace_paths<DB: Db + DbInputs>(
    db: &mut DB,
    paths: &[PathBuf],
    editor_owned: &HashSet<UrlId>,
) {
    let new: Vec<(PathBuf, UrlId)> = paths
        .iter()
        .filter_map(|p| {
            let url = UrlId::from_file_path(p).ok()?;
            Some((p.clone(), url))
        })
        .collect();
    let new_urls: HashSet<UrlId> = new.iter().map(|(_, u)| u.clone()).collect();

    let old: HashMap<UrlId, Root> = db
        .workspace_roots()
        .roots(db)
        .iter()
        .map(|r| (r.path(db).clone(), *r))
        .collect();

    // Evict roots not in the new set. Editor-owned files survive in
    // `OrphanRoot` so their buffers stay analysable. Everything else goes
    // to `StaleRoot` for entity reuse on re-add.
    for (old_url, &old_root) in &old {
        if !new_urls.contains(old_url) {
            old_root.set_stale(db, Some(editor_owned));
        }
    }

    // Build the new roots list: reuse the existing `Root` for unchanged paths
    // (no rescan, the watcher is the path for in-folder changes), scan the
    // rest.
    let mut new_roots = Vec::with_capacity(new.len());
    for (path, url) in new {
        let root = match old.get(&url) {
            Some(&r) => r,
            None => scan_new_workspace_path(db, &path, url),
        };
        new_roots.push(root);
    }
    db.workspace_roots().set_roots(db).to(new_roots);
}

/// Initial scan of a path that wasn't previously a workspace root. Walks the
/// directory tree, calls `set_package` per discovered package, sets scripts.
fn scan_new_workspace_path<DB: Db + DbInputs>(db: &mut DB, path: &Path, url: UrlId) -> Root {
    let root = Root::new(db, url, RootKind::Workspace, Vec::new(), Vec::new());
    rescan_into(db, root, path);
    root
}

/// Re-run the workspace scan against an existing root. Used as the
/// fallback for events (DESCRIPTION add / remove / edit) that can
/// change the set of packages under the root.
pub(crate) fn rescan_workspace_root<DB: Db + DbInputs>(db: &mut DB, root: Root) {
    let Ok(path) = root.path(db).to_file_path() else {
        log::warn!("Skipped rescan: root URL is not a file path");
        return;
    };
    rescan_into(db, root, &path);
}

fn rescan_into<DB: Db + DbInputs>(db: &mut DB, root: Root, path: &Path) {
    let (packages, scripts) = scan_workspace(path);

    let package_entities: Vec<Package> = packages
        .into_iter()
        .map(|pkg| {
            root.set_package(
                db,
                pkg.description_url,
                pkg.name,
                pkg.version,
                pkg.namespace,
                pkg.files,
                pkg.scripts,
                pkg.collation,
            )
        })
        .collect();

    root.set_packages(db).to(package_entities);
    root.set_workspace_scripts(db, scripts);
}
