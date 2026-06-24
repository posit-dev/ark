use std::collections::BTreeSet;

use crate::workspace_files;
use crate::Db;
use crate::Package;
use crate::RootKind;

/// Packages the workspace depends on
///
/// Returned sorted on package name and unique, maximizing backdating potential.
///
/// Unknown packages (not installed) are dropped, along with those that resolve to a
/// workspace package (which is currently being worked on).
#[salsa::tracked(returns(ref))]
pub fn workspace_dependencies(db: &dyn Db) -> Vec<Package> {
    // Split into several queries to maximize backdating (changing a workspace file
    // shouldn't require re-querying `DESCRIPTION` dependencies)
    let from_files = workspace_file_dependencies(db);
    let from_packages = workspace_package_dependencies(db);

    let mut packages: Vec<Package> = Vec::with_capacity(from_files.len() + from_packages.len());
    packages.extend(from_files);
    packages.extend(from_packages);

    packages.sort_by_cached_key(|package| package.name(db));
    packages.dedup_by_key(|package| package.name(db));

    packages
}

/// Packages used by files / scripts in the workspace
///
/// i.e. `library()` / `require()` / `::` / `:::`
///
/// Returned sorted on package name and unique, maximizing backdating potential.
#[salsa::tracked(returns(ref))]
fn workspace_file_dependencies(db: &dyn Db) -> Vec<Package> {
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

/// Packages used by a workspace package's `DESCRIPTION` file
///
/// We take `Depends` and `Imports`, but not `Suggests`, as `Suggests` will
/// instead be referenced by `::` elsewhere as required
///
/// Returned sorted on package name and unique, maximizing backdating potential.
#[salsa::tracked(returns(ref))]
fn workspace_package_dependencies(db: &dyn Db) -> Vec<Package> {
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
