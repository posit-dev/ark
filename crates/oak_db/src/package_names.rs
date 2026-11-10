use std::collections::BTreeMap;

use crate::BindingKind;
use crate::Bindings;
use crate::Db;
use crate::Name;
use crate::NamespaceVisibility;
use crate::Package;

/// The names a package's namespace reaches, from [`Package::names()`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PackageNames {
    /// The names with their kinds, sorted by name.
    pub bindings: Bindings,
    /// Packages that re-export classification can read, in first-encounter
    /// order, excluding this package. Includes `importFrom()` sources and
    /// their re-export chains. Pending sources or NAMESPACE changes in these
    /// packages can change the kinds in `bindings`.
    pub reexport_sources: Vec<Package>,
    /// The names in `bindings` that only lazy data provides, sorted. An
    /// attached package makes them visible, while a NAMESPACE `import()`
    /// brings in exports only.
    pub lazy_data_only: Vec<String>,
}

#[salsa::tracked]
impl Package {
    /// The names `pkg::name` (`Exported`) or `pkg:::name` (`Internal`) can
    /// reach in this package, with their kinds. The enumerating counterpart of
    /// [`Package::resolve()`], which answers one name at a time.
    ///
    /// - `Exported` lists explicit `NAMESPACE` exports, the package's own
    ///   bindings matching `exportPattern()`, and its lazy data objects
    ///   ([`Package::lazy_data()`]). Patterns do not apply to imports.
    ///   `Package::resolve()` does not consider lazy data, which has no
    ///   definition to resolve to.
    /// - `Internal` lists internal bindings and explicit exports, but not lazy
    ///   data or the imports environment. Explicit exports remain available
    ///   without fetched sources, except for `importFrom()` re-exports.
    /// - `base` has no `NAMESPACE`, so every indexed binding is exported.
    ///
    /// Kinds come from [`Package::bindings()`]. An export with no local
    /// binding takes the merged kind of its `importFrom()` source's
    /// definitions, or unknown when none resolve. Lazy data names have unknown
    /// kind unless an export of the same name is listed, which takes
    /// precedence as it does for `pkg::name` lookup. The packages read along
    /// the way are listed in [`PackageNames::reexport_sources`], so callers
    /// can tell when the kinds may still change without following
    /// `importFrom()` chains themselves.
    ///
    /// Cached because completion requests enumerate every attached package.
    /// This query cannot be a cycle's repeated key because resolution and
    /// semantic indexing do not call it. Re-export lookups use
    /// [`Package::resolve()`], which handles re-export cycles, and
    /// [`Package::bindings()`], which does not follow re-exports.
    #[salsa::tracked(returns(ref))]
    pub fn names(self, db: &dyn Db, visibility: NamespaceVisibility) -> PackageNames {
        let bindings = self.bindings(db);
        let is_base = self.name(db) == "base";

        let mut names: BTreeMap<String, BindingKind> = BTreeMap::new();
        let mut reexport_sources = Vec::new();

        if is_base || visibility == NamespaceVisibility::Internal {
            names.extend(bindings.iter().map(|(name, kind)| (name.to_string(), kind)));
        }

        if !is_base {
            let patterns = self.export_patterns(db);
            let matched = bindings.iter().filter(|(name, _)| patterns.is_match(name));
            names.extend(matched.map(|(name, kind)| (name.to_string(), kind)));

            for name in self.namespace(db).exports.iter() {
                let kind = match bindings.get(name) {
                    Some(kind) => kind,
                    // `pkg:::name` does not search the imports environment. A
                    // name re-exported from a bulk `import()` can't be told
                    // apart from one assigned at load time (`utils::osVersion`),
                    // so it stays.
                    None if visibility == NamespaceVisibility::Internal &&
                        self.imported_from(db).contains_key(name) =>
                    {
                        continue;
                    },
                    None => reexport_kind(db, self, name, &mut reexport_sources),
                };
                names.insert(name.to_string(), kind);
            }
        }

        let mut lazy_data_only = Vec::new();
        if visibility == NamespaceVisibility::Exported {
            for name in self.lazy_data(db) {
                if !names.contains_key(name) {
                    names.insert(name.clone(), BindingKind::Unknown);
                    lazy_data_only.push(name.clone());
                }
            }
        }
        lazy_data_only.sort_unstable();

        // Re-export cycles can include this package in its own dependency list.
        reexport_sources.retain(|source| *source != self);

        PackageNames {
            bindings: Bindings::from_entries(names),
            reexport_sources,
            lazy_data_only,
        }
    }
}

fn reexport_kind(
    db: &dyn Db,
    package: Package,
    name: &str,
    sources: &mut Vec<Package>,
) -> BindingKind {
    let Some(source) = package
        .imported_from(db)
        .get(name)
        .and_then(|source| db.package_by_name(source))
    else {
        return BindingKind::Unknown;
    };
    for package in source.reexport_chain(db, name) {
        if !sources.contains(&package) {
            sources.push(package);
        }
    }

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

impl Package {
    /// The packages [`Package::resolve()`] can read for `name`: this package,
    /// then each package `name` is imported from with `importFrom()`,
    /// transitively, in lookup order.
    ///
    /// Pending sources can determine whether resolution stops at a package or
    /// follows its imports, so a caller reporting when its answer can still
    /// change lists every hop. Over-reporting a dependency only delays the
    /// caller's settled answer, but omitting one could let the caller reuse an
    /// answer that later changes.
    ///
    /// A hop is listed even when the importing package also binds `name`, so
    /// resolution would stop there. Checking that would read
    /// [`Package::bindings()`], which parses every file of each package on
    /// the chain, and a lookup that misses walks every attached package.
    pub fn reexport_chain(self, db: &dyn Db, name: &str) -> Vec<Package> {
        let mut chain = vec![self];
        let mut current = self;
        while let Some(next) = current
            .imported_from(db)
            .get(name)
            .and_then(|next| db.package_by_name(next))
        {
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
            current = next;
        }
        chain
    }
}
