use std::sync::Arc;

use aether_url::UrlId;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolTable;
use oak_semantic::use_def_map::UseDefMap;
use stdext::result::ResultExt;

use crate::imports::SalsaImportsResolver;
use crate::parse::OakParse;
use crate::Db;
use crate::Name;
use crate::Package;
use crate::Root;

/// A source file tracked by Salsa.
///
/// Content is pushed into Salsa by the LSP layer, the database never does I/O.
/// This matches rust-analyzer's push model and avoids tying parsing to
/// disk/network I/O inside a Salsa query.
///
/// The `url` field is a [`UrlId`], so the type system enforces "everything
/// inside Salsa is a canonical URL".
///
/// File ownership has a single source of truth: the forward edge. A file
/// belongs to whichever container currently holds it -- `pkg.files`,
/// `root.scripts`, or `orphan_root().files`. There is no stored
/// back-pointer to keep in sync. "What package owns this file?" is
/// answered by the derived [`File::package`] query, which walks the
/// per-root `File -> Package` indexes; "what root?" by [`File::root`].
#[salsa::input(debug)]
pub struct File {
    #[returns(ref)]
    pub url: UrlId,
    #[returns(ref)]
    pub contents: String,
}

#[salsa::tracked]
impl File {
    /// Parse this file's contents into an R syntax tree.
    ///
    /// Crate-internal: kept `pub(crate)` so downstream crates can't take a
    /// dependency on a full parse tree. They reach the tree only through
    /// narrower public queries.
    ///
    /// `lru = 128` caps the number of live parse trees to 128. Matches
    /// rust-analyzer's default for its analogous `parse(file_id)` query.
    /// Rowan's green tree shares structure across edits, so eviction frees
    /// memory cleanly. Derived queries (e.g. `semantic_index`) store
    /// `AstPtr`s rather than tree nodes, so they don't pin an evicted tree.
    #[salsa::tracked(returns(ref), lru = 128)]
    pub(crate) fn parse(self, db: &dyn Db) -> OakParse {
        OakParse::new(aether_parser::parse(
            self.contents(db),
            aether_parser::RParserOptions::default(),
        ))
    }

    /// Build this file's `SemanticIndex` from the parse tree.
    ///
    /// This is a coarse query that invalidates downstream on every edit
    /// (`AstPtr` ranges inside `Definition`s shift). External consumers should
    /// go through the narrow queries: `exports()`, `imports()`, `resolve()`,
    /// `attached_packages()`, `symbol_table()`, `use_def_map()` to shield
    /// themselves from edit changes.
    ///
    /// Cross-file symbol resolution (`source()` injection, NSE resolution)
    /// is driven by [`SalsaImportsResolver`].
    ///
    /// `cycle_result` is required because `source()` cycles produce a
    /// dependency graph through both `semantic_index` and `exports`:
    /// `semantic_index(A) -> SalsaImportsResolver -> exports(B) ->
    /// semantic_index(B) -> SalsaImportsResolver -> exports(A) ->
    /// semantic_index(A)`. Salsa picks one query to break the cycle and
    /// panics with "set cycle_fn/cycle_initial" unless that query has a
    /// handler. Both `semantic_index` and the narrow queries (`exports`,
    /// `imports`, `resolve`) carry their own `cycle_result`.
    ///
    /// The two handlers behave differently:
    ///
    /// - `semantic_index` (this query, custom rebuild): the cycling
    ///   side is rebuilt with `NoopImportsResolver`. Cross-file
    ///   injection drops, but local analysis (scopes, use-def maps,
    ///   function bodies) is preserved.
    ///
    /// - `exports` / `imports` / `resolve` (FallbackImmediate, empty):
    ///   the cycling side gets an empty fallback for that query.
    ///
    /// Which handler fires depends on which query salsa first re-enters.
    /// R doesn't allow cyclic `source()`, so the asymmetric recovery is
    /// acceptable. TODO(diagnostics): Lint `source()` cycles.
    ///
    /// `no_eq` skips salsa's `values_equal` check after recomputation.
    /// Backdating at this level never triggered in practice anyway: `AstPtr`
    /// ranges inside `Definition`s typically shift on edits.
    #[salsa::tracked(returns(ref), no_eq, cycle_result = semantic_index_cycle_result)]
    pub(crate) fn semantic_index(self, db: &dyn Db) -> SemanticIndex {
        build_semantic_index(self, db)
    }

    /// The symbol table for one scope of this file.
    #[salsa::tracked]
    pub fn symbol_table(self, db: &dyn Db, scope: ScopeId) -> Arc<SymbolTable> {
        Arc::clone(self.semantic_index(db).symbols(scope))
    }

    /// The use-def map for one scope of this file.
    #[salsa::tracked]
    pub fn use_def_map(self, db: &dyn Db, scope: ScopeId) -> Arc<UseDefMap> {
        Arc::clone(self.semantic_index(db).use_def_map(scope))
    }

    /// Package names from `library()` / `require()` calls in this file,
    /// including those propagated transitively through `source()` chains.
    /// Ordered by call-site position, which preserves R's search-path
    /// semantics: a later `library(b)` shadows an earlier `library(a)`
    /// when both export the same name.
    #[salsa::tracked(returns(ref))]
    pub fn attached_packages(self, db: &dyn Db) -> Vec<Name<'_>> {
        self.semantic_index(db)
            .attached_packages()
            .into_iter()
            .map(|s| Name::new(db, s))
            .collect()
    }

    /// The [`Package`] that owns this file, or `None` for standalone
    /// scripts and orphan / stale files.
    ///
    /// Derived from live-graph containment via
    /// [`package_by_file_query`](crate::db::package_by_file_query): the
    /// file belongs to whichever live `pkg.files` currently holds it.
    /// This replaces the old `File.package` back-pointer field, so file
    /// ownership has a single source of truth (the forward edge) and no
    /// invariant to maintain by hand.
    #[salsa::tracked]
    pub fn package(self, db: &dyn Db) -> Option<Package> {
        crate::db::package_by_file_query(db, self)
    }

    /// The root containing this file, if any.
    ///
    /// If the file has an owning [`Package`], asks the db which live
    /// root holds it via [`Db::root_by_package`]. Otherwise falls back to a
    /// URL-prefix lookup against [`WorkspaceRoots`] (orphan files live
    /// under a workspace root or nowhere). Library files normally have
    /// a package; the `root_by_package` branch covers them too.
    ///
    /// Returns `None` if the file's package was evicted to
    /// [`StaleRoot`] (no live root contains it), or if the file is in
    /// orphan and the URL falls outside every workspace folder.
    ///
    /// Callers that need to distinguish workspace from library roots
    /// inspect `root.kind(db)`.
    #[salsa::tracked]
    pub fn root(self, db: &dyn Db) -> Option<Root> {
        if let Some(pkg) = self.package(db) {
            return db.root_by_package(pkg);
        }
        root_by_url(db, self.url(db))
    }
}

/// Find the workspace `Root` whose path is the longest-prefix ancestor
/// of `url`. Returns `None` for non-`file:` URLs and for URLs outside
/// every workspace folder. Private helper: the only caller is
/// [`File::root`] (for files without a registered package).
fn root_by_url(db: &dyn Db, url: &UrlId) -> Option<Root> {
    // Virtual documents (e.g. untitled scheme) don't have roots
    if !url.is_file() {
        return None;
    }

    let path = url.to_file_path().log_err()?;
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter_map(|root| {
            let root_path = root.path(db).to_file_path().log_err()?;
            path.starts_with(&root_path).then_some((root_path, *root))
        })
        .max_by_key(|(p, _)| p.components().count())
        .map(|(_, r)| r)
}

fn build_semantic_index(file: File, db: &dyn Db) -> SemanticIndex {
    let parsed = file.parse(db);
    let resolver = SalsaImportsResolver::new(db, file);
    oak_semantic::build_index(&parsed.tree(), resolver)
}

fn semantic_index_cycle_result(db: &dyn Db, _id: salsa::Id, file: File) -> SemanticIndex {
    log::warn!(
        "Cyclic `source()` Detected at {}. Rebuilding without cross-file resolution.",
        file.url(db),
    );
    let parsed = file.parse(db);
    oak_semantic::build_index(&parsed.tree(), oak_semantic::NoopImportsResolver)
}
