use std::collections::BTreeSet;

use crate::workspace_files;
use crate::Db;
use crate::Package;
use crate::RootKind;

/// Packages that any workspace root depends on
///
/// Returned sorted on package name and unique, maximizing backdating potential.
///
/// Unknown packages (not installed) are dropped, along with those that resolve to a
/// workspace package (which is currently being worked on).
#[salsa::tracked(returns(ref))]
pub fn all_package_dependencies(db: &dyn Db) -> Vec<Package> {
    // Split into several queries to maximize backdating (changing a workspace file
    // shouldn't require re-querying `DESCRIPTION` dependencies)
    let from_files = all_workspace_file_dependencies(db);
    let from_packages = all_workspace_package_dependencies(db);
    let from_search = default_search_path_packages(db);

    let mut packages: Vec<Package> =
        Vec::with_capacity(from_files.len() + from_packages.len() + from_search.len());
    packages.extend(from_files);
    packages.extend(from_packages);
    packages.extend(from_search);

    packages.sort_by_cached_key(|package| package.name(db));
    packages.dedup_by_key(|package| package.name(db));

    packages
}

/// Packages used by files / scripts in any workspace root
///
/// i.e. `library()` / `require()` / `::` / `:::`
///
/// Returned sorted on package name and unique, maximizing backdating potential.
#[salsa::tracked(returns(ref))]
fn all_workspace_file_dependencies(db: &dyn Db) -> Vec<Package> {
    // It's likely that we will have a lot of duplicated package use across workspace
    // files, so we use a BTreeSet to avoid having to sort and dedup a large vector
    let mut names: BTreeSet<&str> = BTreeSet::new();

    for &file in workspace_files(db) {
        names.extend(
            file.used_packages(db)
                .iter()
                .map(|name| name.text(db).as_str()),
        );
    }

    as_packages(names, db)
}

/// Packages used by any workspace package's `DESCRIPTION` file
///
/// We take `Depends` and `Imports`, but not `Suggests`, as `Suggests` will
/// instead be referenced by `::` elsewhere as required
///
/// Returned sorted on package name and unique, maximizing backdating potential.
#[salsa::tracked(returns(ref))]
fn all_workspace_package_dependencies(db: &dyn Db) -> Vec<Package> {
    // We aren't expecting there to be many duplicates here (probably none for single
    // package workspaces), so a simple Vec is fine
    let mut names = Vec::new();

    for root in db.workspace_roots().roots(db).iter() {
        for &package in root.packages(db) {
            if let Some(description) = package.description(db) {
                names.extend(description.depends.iter().map(String::as_str));
                names.extend(description.imports.iter().map(String::as_str));
            }
        }
    }

    names.sort();
    names.dedup();

    as_packages(names, db)
}

/// Packages implicitly used by a workspace via R's default search path
///
/// - Workspace scripts always implicitly use these packages, even if they aren't
///   mentioned via a `library()` / `::` call.
///
/// - Workspace package files technically only depend on {base}, but if your package has
///   any arbitrary scripts in it then we are going need the rest of the base packages
///   anyways, and returning them all here just ensures that their sources are available,
///   it doesn't affect package diagnostics, so it's okay to over approximate for
///   simplicity.
///
/// These are effectively static, so we don't need to sort them by name at this point
#[salsa::tracked(returns(ref))]
fn default_search_path_packages(db: &dyn Db) -> Vec<Package> {
    as_packages(crate::search::DEFAULT_SEARCH_PATH_PACKAGES, db)
}

/// Converts an iterable of `names` into their corresponding `Package`s, throwing out:
/// - Non-installed packages (untracked by the db)
/// - Workspace packages (actively being worked on by the user, so `R/` is present
///   already)
fn as_packages<'a>(names: impl IntoIterator<Item = &'a str>, db: &dyn Db) -> Vec<Package> {
    let mut packages = Vec::new();

    for name in names {
        let Some(package) = db.package_by_name(name) else {
            continue;
        };
        let Some(root) = db.root_by_package(package) else {
            continue;
        };
        if root.kind(db) != RootKind::Library {
            continue;
        }
        packages.push(package);
    }

    packages
}
