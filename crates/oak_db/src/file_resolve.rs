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
use crate::PackageVisibility;

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
    /// The returned `Definition` is keyed by `(file, scope, name)`, so
    /// downstream queries that only depend on identity stay cached across
    /// edits that shift the binding's source position. Consumers that need
    /// a position or the bound expression read `def.kind(db)` and project
    /// per-variant.
    ///
    /// For the offset-aware, sequential-semantics variant, see
    /// [`File::resolve_at`].
    #[salsa::tracked]
    pub fn resolve(self, db: &'db dyn Db, name: Name<'db>) -> Option<Definition<'db>> {
        if let Some(def) = self.resolve_export(db, name) {
            return Some(def);
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
                    if let Some(def) = target.resolve_export(db, name) {
                        return Some(def);
                    }
                },
                ImportLayer::Package(pkg) => {
                    if let Some(def) = pkg
                        .resolve(db, name, PackageVisibility::Exported)
                        .into_iter()
                        .next()
                    {
                        return Some(def);
                    }
                },
                ImportLayer::From(map) => {
                    let name_str = name.text(db).as_str();
                    if let Some(pkg_name) = map.get(name_str) {
                        if let Some(pkg) = db.package_by_name(pkg_name) {
                            if let Some(def) = pkg
                                .resolve(db, name, PackageVisibility::Exported)
                                .into_iter()
                                .next()
                            {
                                return Some(def);
                            }
                        }
                    }
                },
            }
        }

        None
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
                .filter_map(|(scope, def_id)| self.resolve_definition(db, scope, def_id))
                .collect();
        }

        // Nothing local reaches the use, so resolve across files.
        let file_scope = ScopeId::from(0);
        if use_scope != file_scope {
            // Function body: the lazy / end-of-file view the body sees at run time.
            return self.resolve(db, name).into_iter().collect();
        }

        // Top level: collation predecessors / other visible files (exports-only
        // chase, same as `resolve`'s imports walk). Avoids the sibling cycle and
        // matches R's namespace semantics.
        for layer in self.imports_at(db, offset) {
            match layer {
                ImportLayer::File(target) => {
                    if let Some(def) = target.resolve_export(db, name) {
                        return vec![def];
                    }
                },
                ImportLayer::Package(pkg) => {
                    let defs = pkg.resolve(db, name, PackageVisibility::Exported);
                    if !defs.is_empty() {
                        return defs;
                    }
                },
                ImportLayer::From(map) => {
                    let name_str = name.text(db).as_str();
                    if let Some(pkg_name) = map.get(name_str) {
                        if let Some(pkg) = db.package_by_name(pkg_name) {
                            let defs = pkg.resolve(db, name, PackageVisibility::Exported);
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
    ) -> Option<Definition<'db>> {
        let index = self.semantic_index(db);
        if let DefinitionKind::Import {
            file: target_url,
            name: forwarded,
            ..
        } = index.definitions(scope_id)[def_id].kind()
        {
            let target = db.file_by_path(&FilePath::from_url(target_url))?;
            return target.resolve_export(db, Name::new(db, forwarded.as_str()));
        }
        self.definition(db, scope_id, def_id)
    }

    /// Walk this file's exports chain for `name`, chasing `source()`-forwarded
    /// `Import` entries through target exports until a `Local` is found. Cycles
    /// resolve to `None` via `exports`'s `cycle_fn`.
    #[salsa::tracked]
    pub(crate) fn resolve_export(
        self,
        db: &'db dyn Db,
        name: Name<'db>,
    ) -> Option<Definition<'db>> {
        let mut current_file = self;
        let mut current_name: Rc<str> = Rc::from(name.text(db).as_str());

        // Defensive: cycle through `Import` is prevented upstream by
        // `exports()`'s `cycle_result` (which returns empty for one cycle
        // participant). The `Rc<str>` is cheap to clone (refcount bump).
        let mut visited: HashSet<(File, Rc<str>)> = HashSet::new();

        loop {
            if !visited.insert((current_file, current_name.clone())) {
                log::error!(
                    "Internal error: Cycle through `Import` forwards while resolving \
                    `{current_name}` in {url}.",
                    url = current_file.path(db),
                );
                return None;
            }

            match current_file.exports(db).get(current_name.as_ref())? {
                ExportEntry::Local => {
                    // Look up the file-scope binding through the semantic index
                    // to recover its `def_id`, then fetch the interned
                    // definition. `exports()` returns the last-wins
                    // definitions, so this is the right binding for the name.
                    let index = current_file.semantic_index(db);
                    let def_id = index.exports().get(current_name.as_ref())?.0;
                    return current_file.definition(db, ScopeId::from(0), def_id);
                },

                ExportEntry::Import { file, name } => {
                    current_file = *file;
                    current_name = Rc::from(name.as_str());
                },
            }
        }
    }
}
