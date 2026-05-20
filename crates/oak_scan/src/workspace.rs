//! Workspace scanner. Drives `WorkspaceRoots` from the editor's open
//! folders.
//!
//! For each workspace path, walks the directory tree (honouring
//! `.gitignore`), discovers packages via `DESCRIPTION` files at any
//! depth, and registers them under a `Workspace` root. R files outside
//! any package directory land in `root.scripts`. Existing `Root`,
//! `Package`, and `File` entities are reused where possible (see
//! [`crate::inputs`]).

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

use crate::inputs::RootExt;
use crate::packages::scan_workspace;

/// Scan each workspace path and register the result under
/// `WorkspaceRoots`. Called through
/// [`crate::DbExt::scan_workspace_paths`].
pub(crate) fn scan_workspace_paths<DB: Db + DbInputs>(db: &mut DB, paths: &[PathBuf]) {
    let existing: HashMap<UrlId, Root> = db
        .workspace_roots()
        .roots(db)
        .iter()
        .map(|r| (r.path(db).clone(), *r))
        .collect();

    let mut roots = Vec::with_capacity(paths.len());
    for path in paths {
        match scan_workspace_path(db, path, &existing) {
            Some(root) => roots.push(root),
            None => log::warn!("Skipped workspace path: {}", path.display()),
        }
    }
    db.workspace_roots().set_roots(db).to(roots);
}

fn scan_workspace_path<DB: Db + DbInputs>(
    db: &mut DB,
    path: &Path,
    existing: &HashMap<UrlId, Root>,
) -> Option<Root> {
    let url = UrlId::from_file_path(path).ok()?;
    let root = match existing.get(&url) {
        Some(&r) => r,
        None => Root::new(db, url, RootKind::Workspace, Vec::new(), Vec::new()),
    };

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
                pkg.collation,
            )
        })
        .collect();

    root.set_packages(db).to(package_entities);
    root.set_workspace_scripts(db, scripts);

    Some(root)
}
