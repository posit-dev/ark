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
    let scripts: Vec<File> = root.scripts(db).clone();

    // A root script can point to a package of another root (see
    // `upsert_workspace_scripts()`). That root is rescanned if it survives, so
    // the backpointer is cleared like the others.
    for &file in &scripts {
        file.set_package(db).to(None);
    }
    root.set_scripts(db).to(Vec::new());

    clear_package_contents(db, &packages);
    root.set_packages(db).to(Vec::new());
    root.set_environment_dirs(db).to(Vec::new());
}

/// Empty the `files` (source and generated) and `scripts` of `packages` and
/// clear the backpointers of the files they listed. A package must not list a file whose package
/// association is `None`, and a file must not point to a package that no
/// longer lists it.
///
/// Also empties [`Package::lazy_data()`] and resets
/// [`Package::sources_revision()`] and [`Package::facts_revision()`], so the
/// LSP requests sources and facts again if the package returns to a library
/// root.
///
/// Used when eviction or a library rescan removes packages from their root.
/// The files become invisible to analysis unless another live container lists
/// them. Public entry points keep open buffers visible with
/// [`crate::inputs::keep_open_files_visible()`].
pub(crate) fn clear_package_contents<DB: Db + DbInputs>(db: &mut DB, packages: &[Package]) {
    for &pkg in packages {
        // Empty lists can still have recorded coverage. Reset both revisions
        // so a revived package is eligible for sources and facts again.
        if pkg.sources_revision(db).is_some() {
            pkg.set_sources_revision(db).to(None);
        }
        if pkg.facts_revision(db).is_some() {
            pkg.set_facts_revision(db).to(None);
        }
        pkg.set_lazy_data(db, Vec::new());

        let files: Vec<File> = pkg
            .files(db)
            .iter()
            .chain(pkg.scripts(db))
            .copied()
            .collect();
        if files.is_empty() {
            continue;
        }
        for file in files {
            file.set_package(db).to(None);
        }
        pkg.clear_files(db);
        if !pkg.scripts(db).is_empty() {
            pkg.set_scripts(db).to(Vec::new());
        }
    }
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
