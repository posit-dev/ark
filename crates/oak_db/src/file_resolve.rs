use std::collections::HashSet;
use std::rc::Rc;

use biome_rowan::TextSize;
use oak_semantic::DefinitionId;
use oak_semantic::ScopeId;

use crate::Db;
use crate::Definition;
use crate::ExportEntry;
use crate::File;
use crate::ImportLayer;
use crate::Name;

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
    /// 2. **`imports()` walk**: each `ImportLayer::File` sibling is checked
    ///    via its own exports chain only (not its full `resolve`). Sibling
    ///    package files would otherwise cycle through each other's
    ///    `imports`, and R's namespace semantics don't transitively include
    ///    siblings' imports anyway. Package-level layers (`From`,
    ///    `Package`) are deferred to PR 4.
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
        //
        // TODO(sources): consume `From` and `Package` layers here. Today
        // resolve only walks `ImportLayer::File`.
        // Requires materializing installed-package files as `oak_db::File`
        // entities first.
        for layer in self.imports(db) {
            if let ImportLayer::File(target) = layer {
                if let Some(def) = target.resolve_export(db, name) {
                    return Some(def);
                }
            }
        }

        None
    }

    /// Resolve the name at `offset` to its definition.
    ///
    /// Implements R's full lookup chain at the use site, with offset-aware
    /// sequential semantics for top-level code:
    ///
    /// 1. **Lexical scopes**: walk ancestor scopes from the use, returning
    ///    the first function-scope binding (parameter, local `<-`, etc.).
    /// 2. **In a function body (lazy context)**: defer to `resolve`, which
    ///    uses the EOF state (full imports, source forwards). The function
    ///    will run after the whole script/package is loaded, so we
    ///    over-approximate accordingly.
    /// 3. **At top-level (sequential context)**: walk the exports chain
    ///    (handles `<-` and `source()` shadowing via `exports`'s
    ///    last-wins ordering), then walk `imports_at(offset)` for the
    ///    visible subset of attaches / collation predecessors.
    ///
    /// Not `#[salsa::tracked]` because keying on `(self, offset)` would
    /// balloon the cache. `Definition` creation delegates to the tracked
    /// [`File::resolve_export()`] and [`File::intern_definition()`].
    pub fn resolve_at(self, db: &'db dyn Db, offset: TextSize) -> Option<Definition<'db>> {
        let index = self.semantic_index(db);
        let (use_scope, _use_id, use_site) = index.use_at(offset)?;
        let name = index
            .symbols(use_scope)
            .symbol(use_site.symbol())
            .name()
            .to_string();

        // Step 1, lexical. Function-scope hits return directly. File-scope
        // hits fall through. `resolve_symbol()` only tracks IS_BOUND, but
        // `exports()` orders bindings in source order so steps 2/3 pick the
        // last winner between `<-` and a same-name `source()`.
        let file_scope = ScopeId::from(0);
        if let Some((binding_scope, def_id)) = index.resolve(&name, use_scope) {
            if binding_scope != file_scope {
                return Some(self.intern_definition(
                    db,
                    binding_scope,
                    def_id,
                    Name::new(db, name.as_str()),
                ));
            }
        }

        let in_function = use_scope != file_scope;
        let interned = Name::new(db, name.as_str());

        // 2. Function body: In lazy contexts we over-approximate by resolving
        // as if cursor was at EOF. Defer to `resolve`.
        if in_function {
            return self.resolve(db, interned);
        }

        // 3. Top-level: exports chain (here) offset-narrowed imports (below).
        if let Some(def) = self.resolve_export(db, interned) {
            return Some(def);
        }

        // Same exports-only chase as `resolve`'s imports walk: avoids the
        // sibling cycle, matches R's namespace semantics. TODO: Package-level
        // layers.
        for layer in self.imports_at(db, offset) {
            if let ImportLayer::File(target) = layer {
                if let Some(def) = target.resolve_export(db, interned) {
                    return Some(def);
                }
            }
        }

        None
    }

    /// Walk this file's exports chain for `name`, chasing `source()`-forwarded
    /// `Import` entries through target exports until a `Local` is found. Cycles
    /// resolve to `None` via `exports`'s `cycle_fn`.
    #[salsa::tracked]
    fn resolve_export(self, db: &'db dyn Db, name: Name<'db>) -> Option<Definition<'db>> {
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
                    url = current_file.url(db),
                );
                return None;
            }

            match current_file.exports(db).get(current_name.as_ref())? {
                ExportEntry::Local => {
                    // Fetch exports again, this time through the semantic index
                    // to get the volatile `kind` field that the firewall query
                    // `File::exports()` doesn't expose.
                    let kind = current_file
                        .semantic_index(db)
                        .exports()
                        .get(current_name.as_ref())
                        .map(|def| def.kind().clone())?;

                    let file_scope = ScopeId::from(0);
                    return Some(Definition::new(
                        db,
                        current_file,
                        file_scope,
                        Name::new(db, current_name.as_ref()),
                        kind,
                    ));
                },

                ExportEntry::Import { file, name } => {
                    current_file = *file;
                    current_name = Rc::from(name.as_str());
                },
            }
        }
    }

    /// Intern the salsa-tracked `Definition` entity for a binding identified
    /// by `(scope, def_id)` in this file's semantic index. Wraps
    /// `Definition::new` in a tracked context, which is required to construct
    /// tracked structs.
    #[salsa::tracked]
    fn intern_definition(
        self,
        db: &'db dyn Db,
        scope: ScopeId,
        def_id: DefinitionId,
        name: Name<'db>,
    ) -> Definition<'db> {
        let kind = self.semantic_index(db).definitions(scope)[def_id]
            .kind()
            .clone();
        Definition::new(db, self, scope, name, kind)
    }
}
