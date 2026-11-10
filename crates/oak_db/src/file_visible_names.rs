use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::ScopeId;
use rustc_hash::FxHashMap;

use crate::BindingKind;
use crate::Db;
use crate::Definition;
use crate::File;
use crate::ImportLayer;
use crate::Name;
use crate::NamespaceVisibility;
use crate::Package;

/// The names that can be seen at a position, from [`File::visible_names_at()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleNames<'db> {
    /// One entry per name, ordered by where lookup first finds it.
    pub entries: Vec<VisibleName<'db>>,
    /// The installed packages consulted ([`Package::is_installed()`]), in
    /// lookup order, including those contributing no names yet and those read
    /// to classify a package's re-exports
    /// ([`PackageNames::reexport_sources`]). Pending sources or lazy data
    /// names of these packages can still change the answer.
    ///
    /// [`PackageNames::reexport_sources`]: crate::PackageNames::reexport_sources
    pub packages: Vec<Package>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleName<'db> {
    pub name: Name<'db>,
    /// Possible bindings in lookup order, without flow-sensitive filtering.
    /// Inner scopes come before outer ones, as in [`File::visible_names_at()`].
    /// Within one scope or one file's exports, later assignments come first,
    /// since R finds the latest one that has run. In an eager scope that is
    /// the latest binding before the position. The cursor's own lazy scope
    /// runs in order up to the position, so its bindings before the position
    /// come first, latest first, then its later ones, latest first. An
    /// enclosing lazy scope may have run to its end, so it lists the latest
    /// anywhere in it first. The first binding may not have run at this
    /// position, and later bindings may be shadowed at runtime.
    pub bindings: Vec<VisibleBinding<'db>>,
}

/// One binding a visible name can have, with its [`BindingKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleBinding<'db> {
    /// A binding in this file, a file it sources or a file loaded with it.
    /// A binding in a scope of this file takes its own kind
    /// ([`Definition::binding_kind()`]). A top-level binding in another file,
    /// reached through a `source()` call or that file's exports, takes that
    /// file's kind for the name ([`File::bindings()`]), merged across its
    /// definitions there.
    Definition {
        definition: Definition<'db>,
        kind: BindingKind,
    },
    /// A name `package::name` reaches, with its kind from
    /// [`Package::names()`], made visible by an attach, the search path, or a
    /// NAMESPACE import.
    PackageExport { package: Package, kind: BindingKind },
}

impl VisibleBinding<'_> {
    pub fn kind(&self) -> BindingKind {
        match self {
            VisibleBinding::Definition { kind, .. } |
            VisibleBinding::PackageExport { kind, .. } => *kind,
        }
    }
}

impl File {
    /// The names visible at `offset`, with their possible bindings.
    ///
    /// Visibility is scope-based, not flow-sensitive. The cursor's scope and
    /// its ancestors contribute all their bindings. In an eager scope, such as
    /// the top level or a `local()` body, only bindings that start before and
    /// end at or before `offset` contribute. Lazy scopes, such as function
    /// bodies, include later bindings too.
    ///
    /// Inner scopes come first. The file scope is placed among the loader
    /// environment files in lookup order, so files loaded after this one rank
    /// above it and earlier files rank below it. NAMESPACE imports, attached
    /// packages and the search path follow. These layers come from
    /// [`File::imports_at()`], including contexts from files that source this
    /// one.
    ///
    /// The scope at `offset` follows the same cursor rule as
    /// [`File::imports_at()`], so a cursor that continues an unbraced
    /// function body (`function(x) x|`) or sits in an unfinished function at
    /// the end of the file sees the function's names.
    ///
    /// Not `#[salsa::tracked]`, like [`File::resolve_at()`], so cursor offsets
    /// don't accumulate memos. It reads tracked file-level queries.
    pub fn visible_names_at<'db>(self, db: &'db dyn Db, offset: TextSize) -> VisibleNames<'db> {
        let index = self.semantic_index(db);
        let cursor_scope = self.cursor_scope(db, offset);
        let cursor_is_eager = index.scope_is_eager(cursor_scope);
        let bindings_of = |scope_id: ScopeId| {
            if cursor_is_eager {
                ScopeBindings::Before(offset)
            } else if scope_id == cursor_scope {
                ScopeBindings::RunFirst(offset)
            } else {
                ScopeBindings::All
            }
        };

        let mut names = VisibleNamesBuilder::new(db);

        // The file scope joins the loader's lookup order below, like the
        // file's own top-level definitions do in `resolve_at()`.
        let file_scope = ScopeId::from(0);
        for scope_id in index.ancestor_scope_ids(cursor_scope) {
            if scope_id == file_scope {
                break;
            }
            names.add_scope(self, index, scope_id, bindings_of(scope_id));
        }

        let (layers, own_position) = self.imports_and_own_position_at(db, offset);
        let (environments, rest) = layers.split_at(own_position.min(layers.len()));
        for layer in environments {
            names.add_layer(self, layer);
        }
        names.add_scope(self, index, file_scope, bindings_of(file_scope));
        for layer in rest {
            names.add_layer(self, layer);
        }

        names.finish()
    }
}

struct VisibleNamesBuilder<'db> {
    db: &'db dyn Db,
    entries: Vec<VisibleName<'db>>,
    positions: FxHashMap<Name<'db>, usize>,
    packages: Vec<Package>,
}

impl<'db> VisibleNamesBuilder<'db> {
    fn new(db: &'db dyn Db) -> Self {
        Self {
            db,
            entries: Vec::new(),
            positions: FxHashMap::default(),
            packages: Vec::new(),
        }
    }

    fn add_scope(
        &mut self,
        file: File,
        index: &SemanticIndex,
        scope_id: ScopeId,
        bindings: ScopeBindings,
    ) {
        let symbols = index.symbols(scope_id);

        // Names keep the order of their first binding, and each name's
        // bindings are collected in evaluation order, split into those that
        // have run at the cursor and later ones.
        let mut names: Vec<ScopeName<'_, 'db>> = Vec::new();
        let mut positions: FxHashMap<&str, usize> = FxHashMap::default();
        for (def_id, def) in index.scope_bindings(scope_id) {
            let Some(has_run) = bindings.has_run(def.range()) else {
                continue;
            };
            let name = symbols.symbol(def.symbol()).name();
            let position = *positions.entry(name).or_insert_with(|| {
                names.push(ScopeName {
                    name,
                    run: Vec::new(),
                    later: Vec::new(),
                });
                names.len() - 1
            });
            for definition in file.resolve_definition(self.db, scope_id, def_id) {
                let binding = self.definition_binding(file, definition);
                let entry = &mut names[position];
                if has_run {
                    entry.run.push(binding);
                } else {
                    entry.later.push(binding);
                }
            }
        }

        // `resolve_definition()` puts a `source()` import's runtime winner
        // last too. R finds the latest assignment that has run, so it goes
        // first.
        for ScopeName { name, run, later } in names {
            for binding in run.into_iter().rev().chain(later.into_iter().rev()) {
                self.push(name, binding);
            }
        }
    }

    /// Add the names `layer` binds, following [`resolve_import_layer()`] for
    /// how each kind of layer is read.
    ///
    /// [`resolve_import_layer()`]: crate::file_resolve::resolve_import_layer
    fn add_layer(&mut self, file: File, layer: &ImportLayer) {
        let db = self.db;
        match layer {
            // This file's own names come from its scopes, which respect the
            // position. A sourcing context can list the file among its
            // collation siblings.
            ImportLayer::File(member) if *member == file => {},
            ImportLayer::File(member) => {
                let mut exported: Vec<&str> =
                    member.exports(db).iter().map(|(name, _)| name).collect();
                exported.sort_unstable();
                self.add_file_exports(file, *member, exported);
            },
            ImportLayer::SourcingFile {
                file: sourcing,
                exports_so_far,
            } => {
                let mut exported: Vec<&str> = exports_so_far.names().collect();
                exported.sort_unstable();
                self.add_file_exports(file, *sourcing, exported);
            },
            ImportLayer::Import(package) => self.add_package_names(*package, false),
            ImportLayer::Package(package) => self.add_package_names(*package, true),
            ImportLayer::From(importer) => {
                let mut imports: Vec<(&String, &String)> =
                    importer.imported_from(db).iter().collect();
                imports.sort_unstable();
                for (name, source) in imports {
                    // As in `resolve_import_layer()`, an import from a package
                    // absent from every root binds nothing.
                    let Some(package) = db.package_by_name(source) else {
                        continue;
                    };
                    let names = package.names(db, NamespaceVisibility::Exported);
                    let kind = names.bindings.get(name).unwrap_or(BindingKind::Unknown);
                    self.add_package(package);
                    self.add_packages(&names.reexport_sources);
                    self.push(name, VisibleBinding::PackageExport { package, kind });
                }
            },
        }
    }

    /// Add the bindings `member`'s exports give `names`, as seen from `file`.
    fn add_file_exports(&mut self, file: File, member: File, names: Vec<&str>) {
        let db = self.db;
        for name in names {
            // `resolve_export()` normally puts the binding R uses last.
            for definition in member
                .resolve_export(db, Name::new(db, name))
                .into_iter()
                .rev()
            {
                self.push(name, self.definition_binding(file, definition));
            }
        }
    }

    /// A binding in `file`'s scopes takes its own kind. A binding in another
    /// file is a top-level definition reached through that file's exports, so
    /// it takes the file's cached kind for the name, merged across its
    /// definitions. `Definition::binding_kind()` would load each other file's
    /// tree on every request, which can exceed `File::parse()`'s LRU capacity
    /// in a large package.
    fn definition_binding(&self, file: File, definition: Definition<'db>) -> VisibleBinding<'db> {
        let db = self.db;
        let home = definition.file(db);
        let kind = if home == file {
            definition.binding_kind(db)
        } else {
            home.bindings(db)
                .get(definition.name(db).text(db))
                .unwrap_or(BindingKind::Unknown)
        };
        VisibleBinding::Definition { definition, kind }
    }

    fn add_package_names(&mut self, package: Package, with_lazy_data: bool) {
        let names = package.names(self.db, NamespaceVisibility::Exported);
        self.add_package(package);
        self.add_packages(&names.reexport_sources);
        for (name, kind) in names.bindings.iter() {
            let is_lazy_data_only = names
                .lazy_data_only
                .binary_search_by(|lazy| lazy.as_str().cmp(name))
                .is_ok();
            if is_lazy_data_only && !with_lazy_data {
                continue;
            }
            self.push(name, VisibleBinding::PackageExport { package, kind });
        }
    }

    fn add_package(&mut self, package: Package) {
        if package.is_installed(self.db) && !self.packages.contains(&package) {
            self.packages.push(package);
        }
    }

    fn add_packages(&mut self, packages: &[Package]) {
        for package in packages {
            self.add_package(*package);
        }
    }

    fn push(&mut self, name: &str, binding: VisibleBinding<'db>) {
        let name = Name::new(self.db, name);
        let position = *self.positions.entry(name).or_insert_with(|| {
            self.entries.push(VisibleName {
                name,
                bindings: Vec::new(),
            });
            self.entries.len() - 1
        });
        // Deduplicate without changing lookup order. A name has few bindings.
        let bindings = &mut self.entries[position].bindings;
        if !bindings.contains(&binding) {
            bindings.push(binding);
        }
    }

    fn finish(self) -> VisibleNames<'db> {
        VisibleNames {
            entries: self.entries,
            packages: self.packages,
        }
    }
}

/// Which of a scope's bindings [`VisibleNamesBuilder::add_scope()`] lists,
/// relative to the cursor.
#[derive(Clone, Copy)]
enum ScopeBindings {
    /// Only the bindings before the offset, for every scope when the cursor
    /// is in an eager one.
    Before(TextSize),
    /// Every binding, with those before the offset counting as run: the
    /// cursor's own lazy scope, whose body runs in order up to the cursor.
    RunFirst(TextSize),
    /// Every binding, all counting as run: an enclosing lazy scope, which may
    /// have run to its end by the time the cursor's code runs.
    All,
}

impl ScopeBindings {
    /// Whether a binding at `range` counts as run at the cursor, or `None`
    /// when the scope doesn't list it.
    fn has_run(self, range: TextRange) -> Option<bool> {
        // A `source()` import has an empty range at the call's start, so it
        // counts from just after that point.
        let before = |offset: TextSize| range.start() < offset && range.end() <= offset;
        match self {
            ScopeBindings::Before(offset) => before(offset).then_some(true),
            ScopeBindings::RunFirst(offset) => Some(before(offset)),
            ScopeBindings::All => Some(true),
        }
    }
}

/// A name's bindings in one scope, in evaluation order.
struct ScopeName<'a, 'db> {
    name: &'a str,
    run: Vec<VisibleBinding<'db>>,
    later: Vec<VisibleBinding<'db>>,
}
