//! Eviction logic shared by the library and workspace `set_*_paths` helpers.
//!
//! Eviction changes membership, not identity. Salsa retains the input
//! entities, and [`oak_db::IdentityMap`] retains their path-to-entity mappings
//! so later scans reuse the same `File` and `Package` entities.
//!
//! - **Editor-owned files** -> `OrphanRoot`. The user has a buffer open;
//!   the file should keep showing up in analysis even though the
//!   workspace folder went away. The buffer is the source of truth for
//!   its contents until `didClose`.
//!
//!   Evicted package files have `File.package` set to `None` because
//!   `OrphanRoot` has no package container. Analysis treats them as standalone
//!   scripts. A scan that adds the workspace back reuses the same `File`
//!   through the identity map and restores its package context.
//!
//! - **Everything else** leaves the evicted root's containers. It becomes
//!   invisible to analysis unless a surviving nested root still lists it, in
//!   which case `set_workspace_paths()` rescans that root to repair the
//!   shared packages that eviction emptied.

use std::collections::HashSet;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use salsa::Setter;

/// Empty `root`'s containers and keep its editor-owned files visible through
/// `OrphanRoot`, as described in the module doc.
///
/// `editor_owned` is `None` for callers without an editor concept (the library
/// scanner) and `Some(&set)` for the workspace scanner. Files in `editor_owned`
/// go to `OrphanRoot`.
///
/// Doesn't touch `LibraryRoots` / `WorkspaceRoots`. The caller is responsible
/// for rebuilding those Vec inputs with `root` excluded.
///
/// Exposed through [`crate::RootExt::evict()`].
pub(crate) fn evict_root<DB: Db + DbInputs>(
    db: &mut DB,
    root: Root,
    editor_owned: Option<&HashSet<FilePath>>,
) {
    let packages: Vec<Package> = root.packages(db).clone();
    let all_files = root_files(db, root);

    // The evicted packages are emptied below, so a backpointer to one would
    // lie about containment. Orphan files must have `package == None` anyway.
    for &file in &all_files {
        file.set_package(db).to(None);
    }

    if let Some(owned) = editor_owned {
        let to_orphan: Vec<File> = all_files
            .into_iter()
            .filter(|f| owned.contains(f.path(db)))
            .collect();
        if !to_orphan.is_empty() {
            let orphan = db.orphan_root();
            let mut files = orphan.files(db).clone();
            files.extend(to_orphan);
            orphan.set_files(db).to(files);
        }
    }

    // Empty package containers, including generated files, to match the
    // cleared file backpointers. A package must not list a file whose package
    // association is `None`.
    root.set_scripts(db).to(Vec::new());
    for &pkg in &packages {
        pkg.clear_files(db);
        pkg.set_scripts(db).to(Vec::new());
    }
    root.set_packages(db).to(Vec::new());
    root.set_environment_dirs(db).to(Vec::new());
}

/// Every `File` that `root` lists: its scripts and its packages' files and
/// scripts.
pub(crate) fn root_files<DB: Db + DbInputs>(db: &DB, root: Root) -> Vec<File> {
    let mut files = root.scripts(db).clone();
    for &pkg in root.packages(db) {
        files.extend(pkg.files(db).iter().copied());
        files.extend(pkg.scripts(db).iter().copied());
    }
    files
}
