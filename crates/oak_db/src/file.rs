use std::sync::Arc;

use aether_url::UrlId;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolTable;
use oak_semantic::use_def_map::UseDefMap;

use crate::parse::OakParse;
use crate::resolver::DbResolver;
use crate::root::url_to_root;
use crate::Db;
use crate::Name;
use crate::PackageOrigin;
use crate::Root;
use crate::SourceNode;

/// A source file tracked by Salsa.
///
/// Content is pushed into Salsa by the LSP layer, the database never does I/O.
/// This matches rust-analyzer's push model and avoids tying parsing to
/// disk/network I/O inside a Salsa query.
///
/// The `url` field is a [`UrlId`], so the type system enforces "everything
/// inside Salsa is a canonical URL".
///
/// `parent` is a back-pointer to the file's owner. Inverse of
/// `SourceGraph.scripts` and `Package.collation`, so queries answering
/// "what owns this file?" don't walk the forward edges. `None` for orphan.
#[salsa::input(debug)]
pub struct File {
    #[returns(ref)]
    pub url: UrlId,
    #[returns(ref)]
    pub contents: String,
    pub parent: Option<SourceNode>,
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
    /// `pub(crate)` so internal modules (`file_exports`, `file_imports`,
    /// `file_resolve`) can read slices of the aggregate to build their
    /// own narrow tracked queries on top. External consumers must go
    /// through the narrow queries: `exports`, `imports`, `resolve`,
    /// `attached_packages`, `symbol_table`, `use_def_map`. Reading the
    /// aggregate invalidates downstream on every edit (`AstPtr` ranges
    /// inside `Definition`s shift).
    ///
    /// Cross-file symbol resolution (`source()` injection, NSE resolution)
    /// is driven by [`DbResolver`].
    ///
    /// `cycle_result` is required even though `File::exports` also has
    /// one. A `source()` cycle forms a dependency graph that runs through
    /// `semantic_index(A) -> DbResolver -> exports(B) -> semantic_index(B)
    /// -> DbResolver -> exports(A) -> semantic_index(A)`, and salsa
    /// panics with "set cycle_fn/cycle_initial" unless the query first
    /// re-entered has a handler. The handler rebuilds the file with
    /// `NoopResolver`, which drops cross-file injection. The cycling
    /// side keeps its own local analysis (scopes, use-def maps, function
    /// bodies); only its source-injected imports from the cycle partner
    /// are missing. R doesn't allow cyclic `source()`, so the asymmetric
    /// outcome (the non-cycling side still sees the cycle partner's
    /// locals) is acceptable.
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
    #[salsa::tracked]
    pub fn attached_packages(self, db: &dyn Db) -> Vec<Name<'_>> {
        self.semantic_index(db)
            .file_attached_packages()
            .into_iter()
            .map(|s| Name::new(db, s))
            .collect()
    }

    /// The workspace root containing this file.
    ///
    /// Workspace-package files return their package's
    /// `PackageOrigin::Workspace { root }`. Installed-package files
    /// return `None`. Other files look up the file's URL against
    /// `WorkspaceRoots`, returning the longest-prefix ancestor or
    /// `None` when the URL is outside every workspace folder.
    ///
    /// Used by `source()` resolution to anchor relative paths against
    /// the project root, matching R's runtime semantics (paths resolve
    /// against `getwd()`, typically the project root in an IDE).
    #[salsa::tracked]
    pub fn workspace_root(self, db: &dyn Db) -> Option<Root> {
        if let Some(SourceNode::Package(pkg)) = self.parent(db) {
            return match pkg.kind(db) {
                PackageOrigin::Workspace { root } => Some(*root),
                PackageOrigin::Installed { .. } => None,
            };
        }
        url_to_root(db, self.url(db))
    }
}

fn build_semantic_index(file: File, db: &dyn Db) -> SemanticIndex {
    let parsed = file.parse(db);
    let mut resolver = DbResolver::new(db, file);
    oak_semantic::build_index(&parsed.tree(), file.url(db).as_url(), &mut resolver)
}

fn semantic_index_cycle_result(db: &dyn Db, _id: salsa::Id, file: File) -> SemanticIndex {
    let parsed = file.parse(db);
    oak_semantic::build_index(
        &parsed.tree(),
        file.url(db).as_url(),
        &mut oak_semantic::NoopResolver,
    )
}
