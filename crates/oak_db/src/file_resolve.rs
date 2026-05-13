use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::SymbolId;
use oak_semantic::ScopeId;

use crate::Db;
use crate::ExportEntry;
use crate::File;
use crate::ImportLayer;
use crate::Name;

/// The result of resolving a name to a concrete definition.
///
/// Carries `(File, name, range)`. The range is read from the resolved file's
/// `semantic_index`. Consumers (goto-def) can navigate directly to
/// `(file, range)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub file: File,
    pub name: String,
    pub range: TextRange,
}

#[salsa::tracked]
impl File {
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
    ///    siblings' imports anyway. Package-level layers (`PackageImports`,
    ///    `PackageExports`) are deferred to PR 4.
    ///
    /// For the offset-aware, sequential-semantics variant, see
    /// [`File::resolve_at`].
    #[salsa::tracked]
    pub fn resolve(self, db: &dyn Db, name: Name<'_>) -> Option<Resolution> {
        if let Some(res) = self.resolve_in_exports(db, name) {
            return Some(res);
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
        // `imports()` directly, as `PackageImports` / `PackageExports`
        // layers (TODO) so finding them does not require walking through
        // siblings.
        for layer in self.imports(db) {
            if let ImportLayer::File(target) = layer {
                if let Some(res) = target.resolve_in_exports(db, name) {
                    return Some(res);
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
    /// Plain method rather than `#[salsa::tracked]`: tracking would key the
    /// cache on `(self, offset)`, creating one entry per cursor position.
    /// Skipping the cache is fine because the body just reads already-cached
    /// subqueries (`semantic_index`, `exports`, `resolve`, `imports`) and
    /// applies a bounded chain walk.
    pub fn resolve_at(self, db: &dyn Db, offset: TextSize) -> Option<Resolution> {
        let index = self.semantic_index(db);
        let (use_scope, use_id, _) = index.use_at(offset)?;
        let use_symbol = index.uses(use_scope)[use_id].symbol();
        let name = index
            .symbols(use_scope)
            .symbol(use_symbol)
            .name()
            .to_string();

        // 1. Lexical: function-scope hits (parameters, locals inside a
        // function) return directly. File-scope hits deliberately fall
        // through because `resolve_symbol` only tracks IS_BOUND and can't
        // tell whether a `<-` or a same-name `source()` came last, whereas
        // `exports()` orders them in source order (last-one-wins). So we let
        // steps 2/3 decide for file-scope names. Safe because every IS_BOUND
        // file-scope symbol also appears in `binding_events` (what `exports()`
        // reads).
        let file_scope = ScopeId::from(0);
        if let Some((binding_scope, binding_symbol)) = index.resolve_symbol(&name, use_scope) {
            if binding_scope != file_scope {
                let range = self.local_binding_range(db, binding_scope, binding_symbol)?;
                return Some(Resolution {
                    file: self,
                    name,
                    range,
                });
            }
        }

        let (cursor_scope, _) = index.scope_at(offset);
        let in_function = cursor_scope != file_scope;
        let interned = Name::new(db, name);

        // 2. Function body: In lazy contexts we over-approximate by resolving
        // as if cursor was at EOF. Defer to `resolve`.
        if in_function {
            return self.resolve(db, interned);
        }

        // 3. Top-level: exports chain (here) offset-narrowed imports (below).
        if let Some(res) = self.resolve_in_exports(db, interned) {
            return Some(res);
        }

        // Same exports-only chase as `resolve`'s imports walk: avoids the
        // sibling cycle, matches R's namespace semantics. TODO: Package-level
        // layers.
        for layer in self.imports_at(db, offset) {
            if let ImportLayer::File(target) = layer {
                if let Some(res) = target.resolve_in_exports(db, interned) {
                    return Some(res);
                }
            }
        }

        None
    }

    /// Walk this file's exports chain for `name`. Iterates
    /// `ExportEntry::Import` (source-forwarded) entries through each target's
    /// exports until it hits a `Local`. Stays entirely in the "exports
    /// world", never invoking the full [`File::resolve`], so callers in the
    /// imports walk can use this without forming a sibling cycle. Cycles in
    /// `source()` forwarding are handled by `exports`'s `cycle_fn` (cycling
    /// files resolve to empty exports, so the loop terminates with `None`).
    fn resolve_in_exports(self, db: &dyn Db, name: Name<'_>) -> Option<Resolution> {
        let mut current_file = self;
        let mut current_name = name.text(db).to_string();

        loop {
            let entry = current_file.exports(db).get(&current_name)?.clone();
            match entry {
                ExportEntry::Local => {
                    let range = current_file.local_definition_range(db, &current_name)?;
                    return Some(Resolution {
                        file: current_file,
                        name: current_name,
                        range,
                    });
                },
                ExportEntry::Import { script, name } => {
                    current_file = script.file(db);
                    current_name = name;
                },
            }
        }
    }

    /// Range of the first definition of `symbol` in `scope` (in source
    /// order). Used for function-scope local bindings; file-scope locals
    /// go through [`Self::local_definition_range`] which keys by name.
    fn local_binding_range(
        self,
        db: &dyn Db,
        scope: ScopeId,
        symbol: SymbolId,
    ) -> Option<TextRange> {
        self.semantic_index(db)
            .definitions(scope)
            .iter()
            .find(|(_, d)| d.symbol() == symbol)
            .map(|(_, d)| d.range())
    }

    /// Range of the first top-level local definition for `name` in this
    /// file's semantic index. Returns `None` if the name doesn't appear
    /// (defensive; shouldn't happen for a `Local` exports entry).
    fn local_definition_range(self, db: &dyn Db, name: &str) -> Option<TextRange> {
        self.semantic_index(db).file_exports().get(name).copied()
    }
}
