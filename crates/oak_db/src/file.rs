use std::collections::BTreeSet;
use std::fs;
use std::sync::Arc;

use aether_path::FilePath;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolTable;
use oak_semantic::use_def_map::UseDefMap;

use crate::db::root_by_file;
use crate::file_revision::report_untracked_if_zero;
use crate::imports::SalsaImportsResolver;
use crate::parse::OakParse;
use crate::Db;
use crate::FileRevision;
use crate::Name;
use crate::Package;
use crate::Root;

/// A source file tracked by Salsa.
///
/// The file's content is not stored directly. Instead, `source_text()` is a
/// lazy tracked query that returns either the editor's in-memory buffer
/// (`source_text_override`) or reads the file from disk. The `revision` input
/// drives cache invalidation for the disk-read path: bumping it forces
/// `source_text()` to re-read the file without storing the bytes as a salsa
/// input.
///
/// The `path` field is a [`FilePath`], so the type system enforces "everything
/// inside Salsa is a canonical path".
///
/// `package` is a back-pointer to the [`Package`] this file belongs to, or
/// `None` for standalone scripts. Inverse of `Package.files`, so queries
/// answering "what package owns this file?" don't walk the forward edge.
/// Files with `package == None` are either standalone scripts under a
/// workspace root or orphan files not registered anywhere.
///
/// # Placement invariant
///
/// `File.package` and the file's physical location in a `Vec<File>` are
/// expected to agree. A file with `package == Some(pkg)` should live in
/// `pkg.files`. A file with `package == None` should live in either some
/// `root.scripts` or `orphan_root().files`. The salsa setters (`set_path`,
/// `set_revision`, `set_source_text_override`, `set_package`) are `pub` because
/// field visibility couples to setter visibility in salsa but calling
/// `set_package` directly leaves the file in its old bucket and silently breaks
/// this invariant.
///
/// The scanner crate (`oak_scan`) wraps these setters in helpers that
/// maintain placement (move the file between `pkg.files`,
/// `root.scripts`, and `orphan_root().files` as `package` changes).
/// Callers that go around the helpers and use the salsa setters
/// directly must maintain placement themselves.
#[salsa::input(debug)]
pub struct File {
    #[returns(ref)]
    pub path: FilePath,
    pub revision: FileRevision,
    #[returns(ref)]
    pub source_text_override: Option<String>,
    /// **Placement invariant.** Call this setter only through
    /// `oak_scan`'s helpers; see the type-level doc above.
    pub package: Option<Package>,
}

#[salsa::tracked]
impl File {
    /// The file's source text. Returns the editor's in-memory buffer if one is
    /// set (`source_text_override`), otherwise reads from disk.
    ///
    /// Tracked and LRU-bounded so the string isn't pinned forever the way an
    /// input field would be. The body reads `revision` and
    /// `source_text_override` through `db`, so salsa invalidates this memo when
    /// the watcher bumps the revision or the editor sets/clears the override,
    /// and a re-read picks up the new bytes.
    ///
    /// A virtual path or an unreadable file yields empty text (matches ty).
    #[salsa::tracked(returns(ref), lru = 128)]
    pub fn source_text(self, db: &dyn Db) -> String {
        if let Some(text) = self.source_text_override(db) {
            return text.clone();
        }

        // Depend on `revision()` so a bump forces a re-read
        report_untracked_if_zero(db, self.revision(db));

        let FilePath::File(path) = self.path(db) else {
            // Our virtual documents (e.g. untitled://) are push-based, the
            // editor writes them to the source override field via
            // `upsert_editor`. If we ever do lazy virtual documents, that is
            // where we'd hook them up. Until then it would be unexpected to
            // reach here.
            log::warn!("Can't read virtual document `{}`", self.path(db));
            return String::new();
        };

        match fs::read_to_string(path.as_path().as_std_path()) {
            Ok(text) => text,
            Err(err) => {
                // A file we were asked to analyze but can't read (permissions,
                // transient I/O) becomes empty source. Log so the failure isn't
                // wholly silent, otherwise downstream diagnostics and symbols
                // would just come back empty with no explanation.
                log::error!("Failed to read `{path}`: {err:?}");
                String::new()
            },
        }
    }

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
            self.source_text(db).as_str(),
            aether_parser::RParserOptions::default(),
        ))
    }

    /// Line index for this file, mapping byte offsets to `(line, column)`.
    ///
    /// Computed straight from `source_text()`, so it doesn't depend on a syntax
    /// tree. The LSP needs it for every offset <-> position translation. No
    /// `lru`: it's small and almost every request needs it, so it stays
    /// resident for the file's lifetime. Follows ty and rust-analyzer, which
    /// both compute the line index as a query off the source text rather than
    /// storing it on the file input.
    #[salsa::tracked(returns(ref))]
    pub fn line_index(self, db: &dyn Db) -> biome_line_index::LineIndex {
        biome_line_index::LineIndex::new(self.source_text(db).as_str())
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

    /// Package names from `::` / `:::` accesses in this file
    ///
    /// Packages are sorted by name and are unique. This maximizes the ability to
    /// backdate after small file edits.
    #[salsa::tracked(returns(ref))]
    fn namespace_accessed_packages(self, db: &dyn Db) -> Vec<Name<'_>> {
        // Likely that there are many `::` accesses for the same package within a single
        // file, so it's useful to build as a BTreeSet that automatically handles sorting
        // and deduplicating for us, rather than building a long `Vec` of duplicated
        // names.
        let names: BTreeSet<&str> = self
            .semantic_index(db)
            .namespaced_accesses()
            .iter()
            .map(|access| access.package())
            .collect();

        names
            .into_iter()
            .map(|package| Name::new(db, package))
            .collect()
    }

    /// All packages used in this file
    ///
    /// Sources:
    /// - [Self::attached_packages()], i.e. `library()` or `require()`
    /// - [Self::namespace_accessed_packages()], i.e. `::` or `:::`
    ///
    /// Packages are sorted by name and are unique. This maximizes the ability to
    /// backdate after small file edits.
    #[salsa::tracked(returns(ref))]
    pub fn used_packages(self, db: &dyn Db) -> Vec<Name<'_>> {
        let mut names: Vec<Name<'_>> = Vec::new();
        names.extend(self.attached_packages(db));
        names.extend(self.namespace_accessed_packages(db));
        names.sort_by_cached_key(|package| package.text(db));
        names.dedup();
        names
    }

    /// The root containing this file, if any.
    ///
    /// Packaged files ask the db which live root holds the package via
    /// [`Db::root_by_package`]. That branch covers library files too, which
    /// normally have a package. It also keeps the common case cheap: it
    /// depends on each root's package list, not its full file set.
    ///
    /// Unpackaged files go through `root_by_file()`, the deepest root whose
    /// scan actually reached the file. If no scan reached it (an editor
    /// buffer opened before any scan, so it sits in orphan), we fall back
    /// to a URL-prefix lookup so the file still resolves to the workspace
    /// folder it lives under.
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
        root_by_file(db, self).or_else(|| root_by_path(db, self.path(db)))
    }
}

/// Find the workspace `Root` whose path is the longest-prefix ancestor
/// of `path`. Returns `None` for virtual documents and for paths outside
/// every workspace folder. Private helper: the only caller is
/// [`File::root`], as the fallback for an orphan file no scan has reached
/// yet (path prefix is all we have until a scan lands).
fn root_by_path(db: &dyn Db, path: &FilePath) -> Option<Root> {
    // Virtual documents (e.g. untitled scheme) don't have roots
    let path = path.as_path()?;
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter_map(|root| {
            let root_path = root.path(db).as_path()?;
            path.starts_with(root_path).then_some((root_path, *root))
        })
        .max_by_key(|(root_path, _)| root_path.components().count())
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
        file.path(db),
    );
    let parsed = file.parse(db);
    oak_semantic::build_index(&parsed.tree(), oak_semantic::NoopImportsResolver)
}
