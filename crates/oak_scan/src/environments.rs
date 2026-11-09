//! Environment writers own the merged [`LibraryRoots`](oak_db::LibraryRoots).
//!
//! After updating environment state, each writer calls
//! [`finish_environment_write()`] to maintain the library list, evict roots
//! that no live environment lists, and keep open buffers visible. Roots appear
//! once, in order of first occurrence across workspace environments, then the
//! default. Within each environment, roots follow `.libPaths()` lookup order.
//! Writers don't report whether the list changed. Consumers that mirror it,
//! such as library watchers, compare their state with `LibraryRoots` instead.
//!
//! Environments reporting the same library path share one [`Root`]. Once a
//! path leaves the merged list, resolving it again creates a new `Root`.
//! [`crate::ScanScheduler`] checks root identity, so an in-flight rescan of the
//! evicted root is rejected even if the path has returned.

use std::collections::HashMap;
use std::collections::HashSet;
use std::iter;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::Environment;
use oak_db::EnvironmentStatus;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;

use crate::inputs::keep_open_files_visible;
use crate::inputs::RootExt;
use crate::library::list_library;
use crate::library::set_installed_packages;
use crate::library::InstalledPackageEntry;

/// A library path reported by an environment's R session and its installed
/// packages, listed with [`list_library()`].
///
/// Can be built off the main loop without database access.
/// [`crate::DbScan::resolve_environment()`] applies it without filesystem access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryListing {
    pub path: PathBuf,
    pub packages: Vec<InstalledPackageEntry>,
}

/// Directories that need live workspace environments, in library merge order.
///
/// Follows workspace root order, with each root followed by its environment
/// directories in their stored, path-sorted order. A directory shared by roots
/// or listed as both a root and an environment directory appears only at its
/// first occurrence.
///
pub(crate) fn wanted_environment_dirs<DB: Db + DbInputs>(db: &DB) -> Vec<FilePath> {
    let mut seen: HashSet<&FilePath> = HashSet::new();
    let mut dirs = Vec::new();
    for root in db.workspace_roots().roots(db) {
        let root_dirs = iter::once(root.path(db)).chain(root.environment_dirs(db));
        for dir in root_dirs {
            if seen.insert(dir) {
                dirs.push(dir.clone());
            }
        }
    }
    dirs
}

pub(crate) fn reconcile_environments<DB: Db + DbInputs>(db: &mut DB) -> Vec<Environment> {
    let wanted = wanted_environment_dirs(db);
    reconcile_environments_to(db, &wanted)
}

/// Reconcile to an explicit directory list. Tests use it to build
/// environments without workspace roots.
pub(crate) fn reconcile_environments_to<DB: Db + DbInputs>(
    db: &mut DB,
    wanted: &[FilePath],
) -> Vec<Environment> {
    let current: Vec<Environment> = db.workspace_environments().environments(db).clone();
    let by_dir: HashMap<FilePath, Environment> = current
        .iter()
        .filter_map(|&environment| Some((environment.dir(db).clone()?, environment)))
        .collect();

    let mut seen: HashSet<&FilePath> = HashSet::new();
    let mut environments = Vec::with_capacity(wanted.len());
    let mut added = Vec::new();
    for dir in wanted {
        if !seen.insert(dir) {
            continue;
        }
        let environment = match by_dir.get(dir) {
            Some(&environment) => environment,
            None => {
                let environment =
                    Environment::new(db, Some(dir.clone()), EnvironmentStatus::Unresolved, None);
                added.push(environment);
                environment
            },
        };
        environments.push(environment);
    }

    if environments != current {
        // Salsa retains removed environments. Clear their roots so they do
        // not retain references to roots that `sync_library_roots()` evicts.
        for &removed in current.iter().filter(|env| !environments.contains(env)) {
            if removed.status(db) != &EnvironmentStatus::Unresolved {
                removed.set_status(db).to(EnvironmentStatus::Unresolved);
            }
        }
        db.workspace_environments()
            .set_environments(db)
            .to(environments);
    }

    finish_environment_write(db);
    added
}

pub(crate) fn mark_default_environment_unresolved<DB: Db + DbInputs>(db: &mut DB) {
    let default = db.default_environment();
    if default.status(db) != &EnvironmentStatus::Unresolved {
        default.set_status(db).to(EnvironmentStatus::Unresolved);
    }
    if default.last_error(db).is_some() {
        default.set_last_error(db).to(None);
    }
    finish_environment_write(db)
}

pub(crate) fn resolve_environment<DB: Db + DbInputs>(
    db: &mut DB,
    environment: Environment,
    libraries: Vec<LibraryListing>,
) {
    if !environment.is_live(db) {
        return;
    }

    let live: HashMap<FilePath, Root> = db
        .library_roots()
        .roots(db)
        .iter()
        .map(|&root| (root.path(db).clone(), root))
        .collect();

    let mut roots: Vec<Root> = Vec::with_capacity(libraries.len());
    for listing in libraries {
        let Some(path) = FilePath::from_path_buf(listing.path.clone()) else {
            log::warn!(
                "Skipping library path that isn't a file URL: {path}",
                path = listing.path.display()
            );
            continue;
        };
        if roots.iter().any(|root| root.path(db) == &path) {
            continue;
        }
        let root = match live.get(&path) {
            Some(&root) => root,
            None => Root::new(
                db,
                path,
                RootKind::Library,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
        };
        set_installed_packages(db, root, listing.packages);
        roots.push(root);
    }

    let status = EnvironmentStatus::Resolved(roots);
    if environment.status(db) != &status {
        environment.set_status(db).to(status);
    }
    if environment.last_error(db).is_some() {
        environment.set_last_error(db).to(None);
    }
    finish_environment_write(db)
}

pub(crate) fn record_environment_failure<DB: Db + DbInputs>(
    db: &mut DB,
    environment: Environment,
    error: String,
) {
    if !environment.is_live(db) {
        return;
    }
    if environment.last_error(db).as_deref() != Some(error.as_str()) {
        environment.set_last_error(db).to(Some(error));
    }
    finish_environment_write(db);
}

/// List the installed packages of each library path, in order, without
/// database access. Environment jobs call this off the main loop.
pub fn list_libraries(paths: Vec<PathBuf>) -> Vec<LibraryListing> {
    paths
        .into_iter()
        .map(|path| LibraryListing {
            packages: list_library(&path),
            path,
        })
        .collect()
}

pub(crate) fn set_library_paths<DB: Db + DbInputs>(db: &mut DB, paths: &[PathBuf]) {
    let libraries = list_libraries(paths.to_vec());
    let default = db.default_environment();
    resolve_environment(db, default, libraries);
}

fn finish_environment_write<DB: Db + DbInputs>(db: &mut DB) {
    sync_library_roots(db);
    keep_open_files_visible(db);
}

/// Merge roots in [`oak_db::WorkspaceEnvironments`] order, then the default,
/// keeping each root's first occurrence. Evict roots that leave the list.
/// Writes nothing when membership and order are unchanged.
fn sync_library_roots<DB: Db + DbInputs>(db: &mut DB) {
    let mut environments: Vec<Environment> = db.workspace_environments().environments(db).clone();
    environments.push(db.default_environment());

    let mut listed: HashSet<Root> = HashSet::new();
    let mut roots: Vec<Root> = Vec::new();
    for environment in environments {
        for &root in environment.status(db).library_roots() {
            if listed.insert(root) {
                roots.push(root);
            }
        }
    }

    if db.library_roots().roots(db) == &roots {
        return;
    }

    let evicted: Vec<Root> = db
        .library_roots()
        .roots(db)
        .iter()
        .filter(|root| !listed.contains(root))
        .copied()
        .collect();
    for root in evicted {
        // Drop the packages through the library rescan's rule first. It keeps
        // the files of a package that a workspace root adopted when its
        // directory turned from an installed package into a source checkout.
        set_installed_packages(db, root, Vec::new());
        root.evict(db);
    }
    db.library_roots().set_roots(db).to(roots);
}
