use std::collections::HashSet;
use std::rc::Rc;

use aether_path::FilePath;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::DefinitionId;
use oak_semantic::ScopeId;

use crate::file_imports::CollationView;
use crate::Db;
use crate::Definition;
use crate::ExportEntry;
use crate::File;
use crate::ImportLayer;
use crate::Name;
use crate::NamespaceVisibility;
use crate::Package;

/// The definitions from [`File::resolution_at()`], with the packages lookup
/// consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<'db> {
    /// The definitions [`File::resolve_at()`] returns.
    pub definitions: Vec<Definition<'db>>,
    /// The installed packages lookup consulted ([`Package::is_installed()`]),
    /// in lookup order, whether or not they bind the name.
    pub packages: Vec<Package>,
}

#[salsa::tracked]
impl<'db> File {
    /// Resolve `name` against this file's lazy / end-of-file state.
    ///
    /// "Lazy" because this is the binding view a *function body* would see
    /// when it actually runs: by then the file has executed end-to-end, all
    /// `library()` calls have attached, and `source()` chains have been
    /// followed. We can't do full call-site analysis, so we over-approximate
    /// to the EOF state of the file.
    ///
    /// Lookup order:
    /// 1. Loader environments, child before parent, with each environment's
    ///    files in reverse load order. This file contributes its `exports()`
    ///    at its load position, including `source()`-forwarded entries.
    ///    Cycles in `source()` resolve to empty exports via `exports`'s `cycle_result`.
    /// 2. **`loader_fallback()`**: namespace imports, visible attaches, and the
    ///    search-path tail when the loader environments do not definitely bind
    ///    the name.
    /// 3. **`imports_by_sourcing_file()` walk**: one alternative per file that
    ///    sources it, each checked in priority order and the results unioned
    ///    across contexts. `File` siblings are checked via their
    ///    exports chain only (not their full `resolve`), to avoid the cycle
    ///    that recursing would create. `Package` and `From` layers call
    ///    [`Package::resolve`] with `Exported` visibility.
    ///
    /// Conditional bindings retain candidates from lower-priority layers;
    /// a definitely-bound layer ends lookup in that context. Files that source
    /// this one supply alternative contexts, whose results are unioned (see
    /// [`resolve_per_sourcing_file`]).
    ///
    /// Each returned `Definition` is keyed by `(file, scope, name)`, so
    /// downstream queries that only depend on identity stay cached across
    /// edits that shift the binding's source position. Consumers that need
    /// a position or the bound expression read `def.kind(db)` and project
    /// per-variant.
    ///
    /// Uses end-of-file visibility: every collation sibling and only
    /// unconditional top-level attaches are visible, including `library()`
    /// calls after the query's source position. [`File::resolve_at`] applies
    /// position-specific visibility.
    #[salsa::tracked(returns(clone))]
    pub fn resolve(self, db: &'db dyn Db, name: Name<'db>) -> Vec<Definition<'db>> {
        // Depend on `cross_file_layers()` and sibling `exports()` projections,
        // not the `no_eq` semantic index, so position-only edits can backdate
        // this query.
        // Recording packages would read each walked package's `importFrom()`
        // chain and root, dependencies this query's result doesn't need.
        let mut lookup = Lookup::definitions_only();
        let own_bound = self.export_is_bound(db, name);
        let own = self.resolve_export(db, name);
        let environment_bound = resolve_environments(
            db,
            self,
            CollationView::Deferred,
            name,
            &own,
            own_bound,
            &mut lookup,
        );
        if environment_bound && own_bound {
            return lookup.definitions;
        }
        if !environment_bound {
            resolve_import_layers(db, self.loader_fallback(db), name, &mut lookup);
        }
        if !own_bound {
            resolve_per_sourcing_file(db, self.imports_by_sourcing_file(db), name, &mut lookup);
        }
        lookup.definitions
    }

    /// Resolve the name at `offset` to its definition(s).
    ///
    /// Returns every binding that could reach the use, so a name defined on
    /// both arms of an `if`/`else` yields two. A cursor on a binding's own name
    /// resolves to that binding. Empty means nothing reachable binds the name
    /// here.
    ///
    /// Definitions come in lookup order, inner scopes and earlier layers
    /// first. Within one scope they come as the use-def map holds them, in
    /// source order, without singling out the one R would pick at runtime.
    /// [`File::visible_names_at()`] instead puts a scope's latest binding
    /// first.
    ///
    /// Not `#[salsa::tracked]` because keying on `(self, offset)` would
    /// balloon the cache. The `Definition` entities it returns are minted by
    /// the tracked [`File::definitions()`] and looked up via
    /// [`File::definition()`], so they stay cached and identity-stable even
    /// though this lookup isn't.
    pub fn resolve_at(self, db: &'db dyn Db, offset: TextSize) -> Vec<Definition<'db>> {
        let mut lookup = Lookup::definitions_only();
        self.lookup_at(db, offset, &mut lookup);
        lookup.definitions
    }

    /// Like [`File::resolve_at()`], also reporting the installed packages
    /// lookup consulted: the package each NAMESPACE import, attach or
    /// search-path layer reduces the name to, with its re-export chain
    /// ([`Package::reexport_chain()`]), up to the layer where lookup stopped
    /// ([`Package::is_installed()`]). Pending sources of these packages can
    /// still change the definitions, including when there are none yet.
    ///
    /// Not tracked, like [`File::resolve_at()`].
    pub fn resolution_at(self, db: &'db dyn Db, offset: TextSize) -> Resolution<'db> {
        let mut lookup = Lookup::with_packages();
        self.lookup_at(db, offset, &mut lookup);
        Resolution {
            definitions: lookup.definitions,
            packages: lookup.packages.unwrap_or_default(),
        }
    }

    fn lookup_at(self, db: &'db dyn Db, offset: TextSize, lookup: &mut Lookup<'db>) {
        let index = self.semantic_index(db);
        let Some((use_scope, use_id, use_site)) = index.use_at(offset) else {
            // Cursor on a binding's own name (a def site, not a use): jump to
            // it, like rust-analyzer / ty.
            if let Some((scope, def_id, _)) = index.definition_at(offset) {
                lookup.extend(self.definition(db, scope, def_id));
            }
            return;
        };
        let name = index
            .symbols(use_scope)
            .symbol(use_site.symbol())
            .name()
            .to_string();
        let name = Name::new(db, name.as_str());

        // File-scope definitions join the loader's load order. Nested-scope
        // definitions are lexical and keep precedence over that environment.
        let file_scope = ScopeId::from(0);
        let (globals, lexical): (Vec<_>, Vec<_>) = index
            .reaching_definitions(use_scope, use_id)
            .partition(|(scope, _)| *scope == file_scope);

        lookup.definitions.extend(
            lexical
                .into_iter()
                .flat_map(|(scope, def_id)| self.resolve_definition(db, scope, def_id)),
        );

        let own_bound = index.use_is_bound(use_scope, use_id);
        if own_bound && globals.is_empty() {
            return;
        }
        let view = if index.scope_is_eager(use_scope) {
            CollationView::Eager
        } else {
            CollationView::Deferred
        };
        let own: Vec<_> = globals
            .into_iter()
            .flat_map(|(scope, def_id)| self.resolve_definition(db, scope, def_id))
            .collect();
        let environment_bound = resolve_environments(db, self, view, name, &own, own_bound, lookup);
        if environment_bound && own_bound {
            return;
        }
        if !environment_bound {
            resolve_import_layers(db, &self.loader_fallback_at(db, offset), name, lookup);
        }
        if !own_bound {
            resolve_per_sourcing_file(
                db,
                &self.imports_by_sourcing_file_at(db, offset),
                name,
                lookup,
            );
        }
    }

    /// Return the definition at `(scope_id, def_id)`, or, for a `source()`
    /// import, the definitions it forwards to in the sourced file.
    pub(crate) fn resolve_definition(
        self,
        db: &'db dyn Db,
        scope_id: ScopeId,
        def_id: DefinitionId,
    ) -> Vec<Definition<'db>> {
        let index = self.semantic_index(db);
        if let DefinitionKind::Import {
            file: target_url,
            name: forwarded,
            ..
        } = index.definitions(scope_id)[def_id].kind()
        {
            let Some(target) = db.file_by_path(&FilePath::from_url(target_url)) else {
                return Vec::new();
            };
            return target.resolve_export(db, Name::new(db, forwarded.as_str()));
        }
        self.definition(db, scope_id, def_id).into_iter().collect()
    }

    /// Walk this file's exports for `name`, chasing `source()`-forwarded
    /// `Import` entries through target exports until they land on `Local`
    /// definitions. Returns every definition the name reaches, so a name with
    /// two top-level bindings, or forwards from two different sourced files,
    /// yields all of them.
    ///
    /// Order follows `exports()`: entries are visited in definition order and
    /// imports are chased in place, so the last element is normally the
    /// binding R picks at runtime (statements run in order, last write wins).
    /// Several top-level locals collapse to one `Local` marker at the first
    /// one's position, so they come out grouped there rather than each at its
    /// own offset. When conditional locals sit on both sides of a conditional
    /// `source()`, the forwarded definitions then come after a local that
    /// runs later, and the last element isn't the latest write.
    ///
    /// Cycles resolve to empty via `exports`'s `cycle_result`.
    #[salsa::tracked(returns(clone))]
    pub(crate) fn resolve_export(self, db: &'db dyn Db, name: Name<'db>) -> Vec<Definition<'db>> {
        let mut results = Vec::new();

        // `visited` keys on `(file, name)` so each binding is minted once even
        // when several forwards converge on it, and so an `Import` cycle (which
        // `exports()`'s `cycle_result` already breaks) can't loop here. The
        // `Rc<str>` is cheap to clone (refcount bump).
        let mut visited: HashSet<(File, Rc<str>)> = HashSet::new();
        self.collect_exports(
            db,
            Rc::from(name.text(db).as_str()),
            &mut visited,
            &mut results,
        );

        results
    }

    /// Append every definition `name` reaches in `self`'s exports to `results`,
    /// in `exports()` order, recursing into `Import` forwards in place. See
    /// [`File::resolve_export`] for the ordering contract.
    fn collect_exports(
        self,
        db: &'db dyn Db,
        name: Rc<str>,
        visited: &mut HashSet<(File, Rc<str>)>,
        results: &mut Vec<Definition<'db>>,
    ) {
        if !visited.insert((self, name.clone())) {
            return;
        }

        let Some(entries) = self.exports(db).get(name.as_ref()) else {
            return;
        };

        for entry in entries {
            match entry {
                ExportEntry::Local => {
                    // The `Local` marker doesn't carry a `def_id`, so recover
                    // from the semantic index the file-scope `def_id`s still in
                    // effect once the file has run top to bottom, and mint each
                    // through the single site. Usually that's one def; a name
                    // still bound on several paths (both arms of a top-level
                    // `if`/`else`) fans out to several here.
                    //
                    // `exports()` also lists the `Import`-kind defs that
                    // `source()` emits at file scope. Skip them: they're the
                    // forwards already chased through the `Import` entries, and
                    // minting one here would add a bogus target at the empty
                    // `source()` call span.
                    let index = self.semantic_index(db);
                    for (def_id, def) in index.export(name.as_ref()) {
                        if matches!(def.kind(), DefinitionKind::Import { .. }) {
                            continue;
                        }
                        results.extend(self.definition(db, ScopeId::from(0), def_id));
                    }
                },

                ExportEntry::Import {
                    file,
                    name: forwarded,
                } => {
                    file.collect_exports(db, Rc::from(forwarded.as_str()), visited, results);
                },
            }
        }
    }
}

/// Walk loader environments without resolving the current file recursively.
/// Its contribution comes from the caller's position-specific reaching
/// definitions or EOF exports, not from a full-file lookup.
fn resolve_environments<'db>(
    db: &'db dyn Db,
    file: File,
    view: CollationView,
    name: Name<'db>,
    own: &[Definition<'db>],
    own_bound: bool,
    lookup: &mut Lookup<'db>,
) -> bool {
    let layers = file.cross_file_layers(db, view);
    for member in layers.environments.lookup_files() {
        if member == layers.current_file {
            lookup.extend(own.iter().copied());
            if own_bound {
                return true;
            }
        } else if resolve_import_layers(db, &[ImportLayer::File(member)], name, lookup) {
            return true;
        }
    }
    false
}

/// Resolve `name` against a single import layer, returning every definition it
/// reaches there (empty means the layer doesn't bind it, so the caller falls
/// through to the next layer). Shared by [`File::resolve`] and
/// [`File::resolve_at`].
///
/// `File` siblings are chased through their exports only, not their full
/// `resolve`: recursing would walk the sibling's own imports, which include
/// *this* file, and salsa would cycle on any unbound name. Exports-only is also
/// what R's namespace semantics asks for, a package's namespace is the merged
/// exports of its collation files.
pub(crate) fn resolve_import_layer<'db>(
    db: &'db dyn Db,
    layer: &ImportLayer,
    name: Name<'db>,
) -> Vec<Definition<'db>> {
    // Reduce the layer to the package it binds `name` to, then resolve against
    // that package's exports. A `File` sibling is the exception, resolved
    // through its own exports chain.
    match layer {
        ImportLayer::File(file) => file.resolve_export(db, name),
        ImportLayer::SourcingFile {
            file,
            exports_so_far,
        } => {
            // Checking the snapshot here rather than inside `resolve_export()`
            // prevents a `semantic_index` read. Since the index is `no_eq` this
            // would compromise the `File::resolve` firewall.
            if !exports_so_far.contains(name.text(db).as_str()) {
                return Vec::new();
            }
            file.resolve_export(db, name)
        },
        // Lazy data has no definition to resolve to, so import and attach
        // layers both resolve exports only.
        ImportLayer::Import(_) | ImportLayer::Package(_) | ImportLayer::From(_) => {
            match layer_package(db, layer, name) {
                Some(package) => package.resolve(db, name, NamespaceVisibility::Exported),
                None => Vec::new(),
            }
        },
    }
}

/// The package a NAMESPACE import, attach or search-path layer binds `name`
/// to. `None` for file layers, and for an `importFrom()` layer that doesn't
/// import `name` or imports it from a package absent from every root.
fn layer_package(db: &dyn Db, layer: &ImportLayer, name: Name) -> Option<Package> {
    match layer {
        ImportLayer::File(_) | ImportLayer::SourcingFile { .. } => None,
        ImportLayer::Import(package) | ImportLayer::Package(package) => Some(*package),
        ImportLayer::From(importer) => {
            let source = importer.imported_from(db).get(name.text(db).as_str())?;
            db.package_by_name(source)
        },
    }
}

/// Resolve `name` in every context of [`File::imports_by_sourcing_file`] and
/// union the results.
///
/// Two files sourcing the same target do not mask each other: they provide
/// alternative contexts. Contexts converging on one binding (a shared sourced
/// file, a search-path package) dedupe.
fn resolve_per_sourcing_file<'db>(
    db: &'db dyn Db,
    contexts: &[Vec<ImportLayer>],
    name: Name<'db>,
    lookup: &mut Lookup<'db>,
) {
    for context in contexts {
        resolve_import_layers(db, context, name, lookup);
    }
}

/// Collect candidates until a layer definitely binds `name`. A conditional
/// file export cannot hide bindings from earlier files or parent environments.
/// Returns whether lookup stopped at a definitely-bound layer.
fn resolve_import_layers<'db>(
    db: &'db dyn Db,
    layers: &[ImportLayer],
    name: Name<'db>,
    lookup: &mut Lookup<'db>,
) -> bool {
    for layer in layers {
        lookup.consult(db, layer, name);
        let defs = resolve_import_layer(db, layer, name);
        if defs.is_empty() {
            continue;
        }
        lookup.extend(defs);
        let bound = match layer {
            ImportLayer::File(file) => file.export_is_bound(db, name),
            _ => true,
        };
        if bound {
            return true;
        }
    }
    false
}

/// What a lookup collects across layers.
struct Lookup<'db> {
    definitions: Vec<Definition<'db>>,
    /// `None` unless the caller reports packages.
    packages: Option<Vec<Package>>,
}

impl<'db> Lookup<'db> {
    fn definitions_only() -> Self {
        Self {
            definitions: Vec::new(),
            packages: None,
        }
    }

    fn with_packages() -> Self {
        Self {
            definitions: Vec::new(),
            packages: Some(Vec::new()),
        }
    }

    fn extend(&mut self, candidates: impl IntoIterator<Item = Definition<'db>>) {
        // `Vec::contains()` preserves lookup order; each layer yields few definitions.
        for def in candidates {
            if !self.definitions.contains(&def) {
                self.definitions.push(def);
            }
        }
    }

    fn consult(&mut self, db: &'db dyn Db, layer: &ImportLayer, name: Name<'db>) {
        let Some(packages) = &mut self.packages else {
            return;
        };
        let Some(package) = layer_package(db, layer, name) else {
            return;
        };
        for package in package.reexport_chain(db, name.text(db)) {
            if package.is_installed(db) && !packages.contains(&package) {
                packages.push(package);
            }
        }
    }
}
