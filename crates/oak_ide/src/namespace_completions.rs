use std::iter;

use oak_db::BindingKind;
use oak_db::Db;
use oak_db::NamespaceVisibility;
use oak_db::Package;

/// The names that complete after `package::` or `package:::`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceCompletions {
    /// Library packages whose pending sources or host-supplied lazy data
    /// names can change this answer. Lists the completed package first, then
    /// its re-export dependencies from [`oak_db::PackageNames`]. Sources can
    /// change item kinds and add internal bindings or `exportPattern()`
    /// matches. Hosts mark the answer incomplete while any listed package
    /// has pending facts.
    ///
    /// Workspace packages are excluded ([`Package::is_installed()`]).
    pub incomplete_until_packages_settle: Vec<Package>,
    /// Sorted by name, without duplicates.
    pub items: Vec<NamespaceItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceItem {
    pub name: String,
    pub kind: BindingKind,
}

/// List the names of `package` visible through `::` (`Exported`) or `:::`
/// (`Internal`), as enumerated by [`Package::names()`]. `None` when no package
/// of that name is known.
pub fn namespace_completions(
    db: &dyn Db,
    package: &str,
    visibility: NamespaceVisibility,
) -> Option<NamespaceCompletions> {
    let package = db.package_by_name(package)?;
    let names = package.names(db, visibility);

    let items = names
        .bindings
        .iter()
        .map(|(name, kind)| NamespaceItem {
            name: name.to_string(),
            kind,
        })
        .collect();

    let incomplete_until_packages_settle = iter::once(package)
        .chain(names.reexport_sources.iter().copied())
        .filter(|package| package.is_installed(db))
        .collect();

    Some(NamespaceCompletions {
        incomplete_until_packages_settle,
        items,
    })
}
