use std::collections::BTreeSet;

use crate::workspace_files;
use crate::Db;
use crate::Package;
use crate::RootKind;

/// Packages the workspace depends on
///
/// Sources:
/// - `library()` / `require()`
/// - `::` / `:::`
/// - Workspace package `DESCRIPTION`s
///
/// Returned set of `Package`s is sorted and unique, maximizing backdating potential.
///
/// Unknown packages (not installed) are dropped, along with those that resolve to a
/// workspace package (which is currently being worked on).
#[salsa::tracked(returns(ref))]
pub fn workspace_dependencies(db: &dyn Db) -> Vec<Package> {
    // Maintains orderedness and uniqueness for us
    let mut names: BTreeSet<&str> = BTreeSet::new();

    // Packages used by workspace scripts / files, i.e. `library()` or `require()` calls,
    // and `::` or `:::` accesses
    for &file in workspace_files(db) {
        names.extend(
            file.used_packages(db)
                .iter()
                .map(|name| name.text(db).as_str()),
        );
    }

    // Packages used by a workspace package's `DESCRIPTION` file
    //
    // We take `Depends` and `Imports`, but not `Suggests`, as `Suggests` will
    // instead be referenced by `::` elsewhere as required
    for root in db.workspace_roots().roots(db).iter() {
        for &package in root.packages(db) {
            if let Some(description) = package.description(db) {
                names.extend(description.depends.iter().map(String::as_str));
                names.extend(description.imports.iter().map(String::as_str));
            }
        }
    }

    // Now look up the actual `Package` by name, throwing out ones that aren't actually
    // installed, and packages that are actively being worked on in the workspace already
    let mut packages = Vec::with_capacity(names.len());

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
