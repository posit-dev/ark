//! Eviction logic shared by the library and workspace `set_*_paths` helpers.
//!
//! Eviction changes membership, not identity. Salsa retains the input
//! entities, and [`oak_db::IdentityMap`] retains their path-to-entity mappings
//! so later scans reuse the same `File` and `Package` entities.
//!
//! Evicted files leave the root's containers and lose their package
//! backpointers. They become invisible to analysis unless a surviving nested
//! root still lists them, in which case `set_workspace_paths()` rescans that
//! root to repair the shared packages that eviction emptied.
//!
//! Public entry points preserve open buffers with
//! [`crate::inputs::keep_open_files_visible()`] after eviction. Buffers outside
//! every live container move to [`OrphanRoot`](oak_db::OrphanRoot), where
//! analysis treats them as standalone scripts. A later scan reuses the same
//! `File` and restores its package context if a package lists it.

use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use salsa::Setter;

/// Empty `root`'s containers and clear the backpointers of the files they
/// listed, as described in the module doc.
///
/// Doesn't touch `LibraryRoots` / `WorkspaceRoots`. The caller is responsible
/// for rebuilding those Vec inputs with `root` excluded.
///
/// Exposed through [`crate::inputs::RootExt::evict()`].
pub(crate) fn evict_root<DB: Db + DbInputs>(db: &mut DB, root: Root) {
    let packages: Vec<Package> = root.packages(db).clone();
    let all_files = root_files(db, root);

    // The evicted packages are emptied below, so a backpointer to one would
    // lie about containment.
    for &file in &all_files {
        file.set_package(db).to(None);
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
