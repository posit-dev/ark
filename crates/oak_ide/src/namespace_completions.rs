use std::collections::BTreeMap;

use oak_db::BindingKind;
use oak_db::Db;
use oak_db::Name;
use oak_db::NamespaceVisibility;
use oak_db::Package;
use oak_db::RootKind;

/// The names that complete after `package::` or `package:::`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceCompletions {
    /// The library package whose pending sources or host-supplied lazy data
    /// names can change this answer. Sources can change item kinds and add
    /// internal bindings or names matching `exportPattern()`. Hosts mark the
    /// answer incomplete until no such fact can still arrive for it.
    /// `None` for workspace packages, whose files are already available and
    /// for which hosts do not currently supply lazy data names.
    pub incomplete_until_package_settles: Option<Package>,
    /// Sorted by name, without duplicates.
    pub items: Vec<NamespaceItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceItem {
    pub name: String,
    pub kind: BindingKind,
}

/// List the names of `package` visible through `::` (`Exported`) or `:::`
/// (`Internal`). `None` when no package of that name is known.
///
/// - `::` lists explicit `NAMESPACE` exports, the package's own bindings
///   matching `exportPattern()`, and its lazy data objects
///   ([`Package::lazy_data()`]). Patterns do not apply to imports.
/// - `:::` lists internal bindings and explicit exports, but does not search
///   lazy data or the imports environment. Explicit exports remain available
///   without fetched sources, except for `importFrom()` re-exports.
/// - `base` has no `NAMESPACE`, so every indexed binding is exported.
///
/// Kinds come from [`Package::bindings()`]. For an export with no local
/// binding, try resolving an `importFrom()` re-export in its source package.
/// If no definitions with known kinds are available, the kind is unknown.
/// Lazy data names have unknown kind unless an export of the same name is
/// already listed. The export takes precedence, matching `pkg::name` lookup.
pub fn namespace_completions(
    db: &dyn Db,
    package: &str,
    visibility: NamespaceVisibility,
) -> Option<NamespaceCompletions> {
    let package = db.package_by_name(package)?;
    let bindings = package.bindings(db);

    let mut items: BTreeMap<&str, BindingKind> = BTreeMap::new();

    if package.name(db) == "base" || visibility == NamespaceVisibility::Internal {
        items.extend(bindings.iter());
    }

    if package.name(db) != "base" {
        let patterns = package.export_patterns(db);
        let matched = bindings.iter().filter(|(name, _)| patterns.is_match(name));
        items.extend(matched);

        for name in package.namespace(db).exports.iter() {
            let kind = match bindings.get(name) {
                Some(kind) => kind,
                // `pkg:::name` does not search the imports environment. A name
                // re-exported from a bulk `import()` can't be told apart from
                // one assigned at load time (`utils::osVersion`), so it stays.
                None if visibility == NamespaceVisibility::Internal &&
                    package.imported_from(db).contains_key(name) =>
                {
                    continue;
                },
                None => reexport_kind(db, package, name),
            };
            items.insert(name, kind);
        }
    }

    if visibility == NamespaceVisibility::Exported {
        for name in package.lazy_data(db) {
            items.entry(name).or_insert(BindingKind::Unknown);
        }
    }

    let items = items
        .into_iter()
        .map(|(name, kind)| NamespaceItem {
            name: name.to_string(),
            kind,
        })
        .collect();

    let is_library = db
        .root_by_package(package)
        .is_some_and(|root| root.kind(db) == RootKind::Library);
    let incomplete_until_package_settles = is_library.then_some(package);

    Some(NamespaceCompletions {
        incomplete_until_package_settles,
        items,
    })
}

// The importing package has no local binding for `name`, so resolve directly
// in the `importFrom()` source package. Merge kinds across all resolved definitions.
fn reexport_kind(db: &dyn Db, package: Package, name: &str) -> BindingKind {
    let Some(source) = package
        .imported_from(db)
        .get(name)
        .and_then(|source| db.package_by_name(source))
    else {
        return BindingKind::Unknown;
    };

    source
        .resolve(db, Name::new(db, name), NamespaceVisibility::Exported)
        .into_iter()
        .map(|definition| {
            let file = definition.file(db);
            let name = definition.name(db).text(db);
            file.bindings(db)
                .get(name.as_str())
                .unwrap_or(BindingKind::Unknown)
        })
        .reduce(BindingKind::merge)
        .unwrap_or(BindingKind::Unknown)
}
