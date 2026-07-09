use std::collections::HashSet;
use std::rc::Rc;

use aether_path::FilePath;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::DefinitionId;
use oak_semantic::ScopeId;

use crate::Db;
use crate::Definition;
use crate::ExportEntry;
use crate::File;
use crate::ImportLayer;
use crate::Name;
use crate::NamespaceVisibility;

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
    /// 1. **`exports()` chain**: file-top-level locals plus
    ///    `source()`-forwarded entries. `ExportEntry::Import` is chased
    ///    through `exports(target)` until it lands on a `Local`. Cycles in
    ///    `source()` resolve to empty exports via `exports`'s `cycle_fn`.
    /// 2. **`imports()` walk**: each layer is checked in priority order.
    ///    `File` siblings are checked via their exports chain only (not their
    ///    full `resolve`), to avoid the cycle that recursing would create.
    ///    `Package` and `From` layers call [`Package::resolve`] with
    ///    `Exported` visibility.
    ///
    /// Returns every definition the name reaches in the first layer that binds
    /// it, so a name with two top-level bindings yields both. The own-file
    /// `exports()` chain shadows imports, matching R: if the file binds the
    /// name at top level we stop there and never fall through to a package.
    ///
    /// Each returned `Definition` is keyed by `(file, scope, name)`, so
    /// downstream queries that only depend on identity stay cached across
    /// edits that shift the binding's source position. Consumers that need
    /// a position or the bound expression read `def.kind(db)` and project
    /// per-variant.
    ///
    /// For the offset-aware, sequential-semantics variant, see
    /// [`File::resolve_at`].
    #[salsa::tracked]
    pub fn resolve(self, db: &'db dyn Db, name: Name<'db>) -> Vec<Definition<'db>> {
        let exported = self.resolve_export(db, name);
        if !exported.is_empty() {
            return exported;
        }

        // For each sibling `ImportLayer::File`, check the target's exports
        // chain only. Recursing into `target.resolve()` would walk the
        // target's imports, which include *this* file (sibling exclusion
        // is per-file), and salsa would cycle on any unbound name.
        //
        // Exports-only is also what R's namespace semantics asks for. A
        // package's namespace is the merged *exports* of its collation
        // files, so "what does sibling B contribute to the namespace?" is
        // exactly "what's in B's exports?". Package-wide NAMESPACE imports
        // and the installed-package search path appear in this file's own
        // `imports()` directly, as `From` / `Package` layers, so finding
        // them does not require walking through siblings.
        for layer in self.imports(db) {
            match layer {
                ImportLayer::File(target) => {
                    let defs = target.resolve_export(db, name);
                    if !defs.is_empty() {
                        return defs;
                    }
                },
                ImportLayer::Package(pkg) => {
                    let defs = pkg.resolve(db, name, NamespaceVisibility::Exported);
                    if !defs.is_empty() {
                        return defs;
                    }
                },
                ImportLayer::From(map) => {
                    let name_str = name.text(db).as_str();
                    if let Some(pkg_name) = map.get(name_str) {
                        if let Some(pkg) = db.package_by_name(pkg_name) {
                            let defs = pkg.resolve(db, name, NamespaceVisibility::Exported);
                            if !defs.is_empty() {
                                return defs;
                            }
                        }
                    }
                },
            }
        }

        Vec::new()
    }

    /// Resolve the name at `offset` to its definition(s).
    ///
    /// Returns every binding that could reach the use, so a name defined on
    /// both arms of an `if`/`else` yields two. A cursor on a binding's own name
    /// resolves to that binding. Empty means nothing reachable binds the name
    /// here.
    ///
    /// Not `#[salsa::tracked]` because keying on `(self, offset)` would
    /// balloon the cache. The `Definition` entities it returns are minted by
    /// the tracked [`File::definitions()`] and looked up via
    /// [`File::definition()`], so they stay cached and identity-stable even
    /// though this lookup isn't.
    pub fn resolve_at(self, db: &'db dyn Db, offset: TextSize) -> Vec<Definition<'db>> {
        let index = self.semantic_index(db);
        let Some((use_scope, use_id, use_site)) = index.use_at(offset) else {
            // Cursor on a binding's own name (a def site, not a use): jump to
            // it, like rust-analyzer / ty.
            return match index.definition_at(offset) {
                Some((scope, def_id, _)) => {
                    self.definition(db, scope, def_id).into_iter().collect()
                },
                None => Vec::new(),
            };
        };
        let name = index
            .symbols(use_scope)
            .symbol(use_site.symbol())
            .name()
            .to_string();
        let name = Name::new(db, name.as_str());

        // Get local definitions for that use
        let reaching: Vec<(ScopeId, DefinitionId)> =
            index.reaching_definitions(use_scope, use_id).collect();

        if !reaching.is_empty() {
            return reaching
                .into_iter()
                .flat_map(|(scope, def_id)| self.resolve_definition(db, scope, def_id))
                .collect();
        }

        // Nothing local reaches the use, so resolve across files.
        let file_scope = ScopeId::from(0);
        if use_scope != file_scope {
            // Function body: the lazy / end-of-file view the body sees at run time.
            return self.resolve(db, name);
        }

        // Top level: collation predecessors / other visible files (exports-only
        // chase, same as `resolve`'s imports walk). Avoids the sibling cycle and
        // matches R's namespace semantics.
        for layer in self.imports_at(db, offset) {
            match layer {
                ImportLayer::File(target) => {
                    let defs = target.resolve_export(db, name);
                    if !defs.is_empty() {
                        return defs;
                    }
                },
                ImportLayer::Package(pkg) => {
                    let defs = pkg.resolve(db, name, NamespaceVisibility::Exported);
                    if !defs.is_empty() {
                        return defs;
                    }
                },
                ImportLayer::From(map) => {
                    let name_str = name.text(db).as_str();
                    if let Some(pkg_name) = map.get(name_str) {
                        if let Some(pkg) = db.package_by_name(pkg_name) {
                            let defs = pkg.resolve(db, name, NamespaceVisibility::Exported);
                            if !defs.is_empty() {
                                return defs;
                            }
                        }
                    }
                },
            }
        }

        Vec::new()
    }

    fn resolve_definition(
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
    /// imports are chased in place, so the last element is the binding R picks
    /// at runtime (statements run in order, last write wins). Several top-level
    /// locals collapse to one `Local` marker, so they come out grouped at that
    /// marker's position rather than each at its own offset, but the runtime
    /// winner is still last.
    ///
    /// Cycles resolve to empty via `exports`'s `cycle_fn`.
    #[salsa::tracked]
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
                    for &(def_id, def) in index.exports().get(name.as_ref()).into_iter().flatten() {
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
