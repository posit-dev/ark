use std::borrow::Cow;
use std::slice;

use biome_rowan::TextSize;
use camino::Utf8Path;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::semantic_index::AttachRegion;
use oak_semantic::semantic_index::ExportsAtSource;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticCall;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticIndex;

use crate::directory::files_in_directory;
use crate::load_context::load_context;
use crate::load_context::LoadContext;
use crate::load_context::SearchPathTail;
use crate::Db;
use crate::File;
use crate::Package;

/// A layer in a file's import chain.
///
/// Carries salsa entity ids (`File`, `Package`) end-to-end. No URL or
/// package-name strings cross out of `oak_db` for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportLayer {
    /// A file whose top level has fully run by the time this layer is read: a
    /// collation predecessor, or a sourcing file seen from a lazy context.
    /// Names are resolved through `file.exports(db)`.
    File(File),
    /// A file that sources the one being resolved, seen as of its `source()`
    /// call. The rest of `file` hadn't run by then, so only `exports_so_far`
    /// counts.
    SourcingFile {
        file: File,
        exports_so_far: ExportsAtSource,
    },
    /// The package whose NAMESPACE declares `importFrom(pkg, name)` entries.
    /// [`Package::import_index`] says which entry, if any, binds a given name.
    From(Package),
    /// A whole package made available, either via NAMESPACE `import(pkg)`,
    /// `library()` / `require()` calls, or the default R search path.
    /// Missing packages are filtered out by `imports`.
    Package(Package),
}

/// The cross-file layers a file sees at load time, split at the point where the
/// file's own `library()` attaches slot in.
///
/// `above` outranks the file's own attaches. It holds sibling and predecessor
/// definitions plus the NAMESPACE imports, the parts R searches before the
/// attached search path. `below` is the rest of the search path: predecessor
/// attaches, the loader's implicit attaches, and `base`.
///
/// The file's own attaches are deliberately left out, so building this never
/// reads the file's own semantic index. That's what lets the resolver call it
/// while that index is still being built (see [`SalsaImportsResolver`]). Each
/// caller splices its own attaches between the two bands: [`File::imports`]
/// reads them from the file's index, the resolver takes them from the builder's
/// flow-ordered set.
///
/// [`SalsaImportsResolver`]: crate::imports::SalsaImportsResolver
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub(crate) struct CrossFileLayers {
    pub above: Vec<ImportLayer>,
    pub below: Vec<ImportLayer>,
}

impl CrossFileLayers {
    /// The layers as a single lookup-ordered list, splicing the file's own
    /// `library()` attaches into the band between the definition/namespace
    /// layers (which outrank them) and the rest of the search path.
    pub(crate) fn lookup_order<'a>(
        &'a self,
        own: &'a [ImportLayer],
    ) -> impl Iterator<Item = &'a ImportLayer> {
        self.above.iter().chain(own).chain(self.below.iter())
    }
}

/// Which of a file's own `library()` attaches a caller sees.
#[derive(Clone, Copy)]
enum AttachView<'a> {
    /// Every attach in the file.
    ///
    /// Over-approximates on two axes. An attach in a function body counts even
    /// though the body may never run, and a conditional one counts even though
    /// its branch may not have been taken.
    Anywhere,
    /// Attaches visible at `offset` in the lazy scope `scope_id`.
    ///
    /// An attach in `scope_id` itself or an enclosing lazy body applies only after its
    /// `library()` call. On the other hand, the lazy view treats an
    /// unconditional top-level attach as visible regardless of position, which
    /// over-approximates this case:
    ///
    /// ```r
    /// f <- function() {
    ///     cli_alert("x")
    /// }
    ///
    /// f()
    /// library(cli)
    /// ```
    ///
    /// `f()` runs before `library(cli)`, so the attach is unavailable at
    /// `cli_alert()`. In the future, call analysis could detect that order.
    ///
    /// Conditional attaches remain limited to their arm, and child or sibling
    /// bodies do not reach `scope_id`.
    Lazy { offset: TextSize, scope_id: ScopeId },
    /// Attaches in eager scopes that are visible at any of `offsets`.
    ///
    /// Attaches in lazy bodies and after every offset are excluded. Multiple
    /// offsets represent distinct `source()` sites, which may have different
    /// attach views.
    ///
    /// ```r
    /// library(pkga)
    /// source("init.R")
    ///
    /// library(pkgb)
    /// source("init.R")
    /// ```
    ///
    /// or
    ///
    /// ```r
    /// library(pkga)
    /// if (cond1) {
    ///   source("init.R")
    /// }
    /// library(pkgb)
    /// ...
    /// if (cond2) {
    ///   source("init.R")
    /// }
    /// ```
    ///
    /// `init.R` runs twice, represented by an offset for each `source()`
    /// call. It sees `pkga` in both cases but only the second run sees `pkgb`.
    Eager(&'a [TextSize]),
}

impl AttachView<'_> {
    fn sees(&self, index: &SemanticIndex, call: &SemanticCall, region: &AttachRegion) -> bool {
        match *self {
            AttachView::Anywhere => true,
            AttachView::Lazy {
                offset,
                scope_id: scope,
            } => match index.enclosing_lazy_scope(call.scope()) {
                // The lazy view treats unconditional top-level attaches as
                // preceding every body. Conditional attaches stay in their arm.
                None => match region {
                    AttachRegion::Unconditional => true,
                    AttachRegion::Conditional { .. } => region.contains(call, offset),
                },
                // A lazy body's attach reaches only that body and its descendants,
                // and only after the call returns.
                Some(unit) => {
                    index
                        .ancestor_scope_ids(scope)
                        .any(|ancestor| ancestor == unit) &&
                        region.contains(call, offset)
                },
            },
            AttachView::Eager(offsets) => {
                index.scope_is_eager(call.scope()) &&
                    offsets.iter().any(|&offset| region.contains(call, offset))
            },
        }
    }
}

/// Layers that a sourcing file makes visible to the sourced file. `file` is the
/// file holding the `source()` call.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub(crate) struct InheritedLayers {
    pub file: File,
    pub layers: CrossFileLayers,
}

/// The point in a package's load at which a file views its collation siblings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CollationView {
    /// Deferred (a function body, or end-of-file): the code runs after the
    /// whole collation has loaded, so every sibling is visible.
    Lazy,
    /// In load order (a top-level statement): only siblings sourced before this
    /// point have loaded, so a name defined later in the collation isn't visible.
    Eager,
}

#[salsa::tracked]
impl File {
    /// Every import layer this file could see, in R's lookup (LIFO) priority
    /// order. Symbols that don't have local bindings (are unbound in the file's
    /// semantic index) can be resolved against these imports.
    ///
    /// Over-approximates: every attach in the file contributes a layer,
    /// including ones in a function body or in a branch that wasn't taken.
    ///
    /// `library()` calls further down the file come earlier in the returned
    /// `Vec`, and collation files later in the package come earlier too. The
    /// first hit in a forward search then matches R's runtime semantics (last
    /// attached / latest sourced wins).
    ///
    /// Offset-independent and stable across cursor moves. Recomputed only when
    /// the file's package membership, NAMESPACE, or this file's semantic calls
    /// actually change. See [`File::imports_at`] for the offset-narrowed subset
    /// of imports.
    #[salsa::tracked(returns(ref))]
    pub fn imports(self, db: &dyn Db) -> Vec<ImportLayer> {
        let layers = self.resolution_layers(db, CollationView::Lazy);
        let own = self.attach_layers(db, AttachView::Anywhere);
        layers.lookup_order(&own).cloned().collect()
    }

    /// Import layers visible at an `offset` in a file:
    ///
    /// - **Cursor in lazy context**: every collation sibling and unconditional
    ///   top-level attach is visible. Attaches from the current or enclosing lazy
    ///   body must precede the cursor, and conditional attaches remain limited to
    ///   the arm that attaches them.
    ///
    /// - **Top-level cursor (script)**: `library()` calls before `offset`,
    ///   most recently attached first. A script in an `R/` directory also
    ///   sees its collation predecessors, most recently sourced first, the
    ///   same convention as a package file and as with `shiny.autoload.r`.
    ///
    /// - **Top-level cursor (package)**: only collation predecessors
    ///   of this file. Most recently sourced predecessor comes
    ///   first. The package imports and base namespace come last.
    ///
    /// Plain method rather than `#[salsa::tracked]`. Tracking would key the
    /// cache on `(self, offset)`, creating one entry per cursor position.
    /// Skipping the cache is fine because the body just reads already-cached
    /// subqueries (`cross_file_layers`, `semantic_index`) and applies an O(n)
    /// filter.
    pub fn imports_at(self, db: &dyn Db, offset: TextSize) -> Vec<ImportLayer> {
        let index = self.semantic_index(db);
        let (cursor_scope, _) = index.scope_at(offset);

        // An eager scope runs during the file's own top-level execution, so a
        // cursor in a `local()` block sees the search path as of that point,
        // the same as one at file scope.
        let (collation, attaches) = if index.scope_is_eager(cursor_scope) {
            // Predecessors only, and own attaches narrowed to the calls that
            // have run by `offset`.
            (
                CollationView::Eager,
                AttachView::Eager(slice::from_ref(&offset)),
            )
        } else {
            (CollationView::Lazy, AttachView::Lazy {
                offset,
                scope_id: cursor_scope,
            })
        };

        let layers = self.resolution_layers(db, collation);
        let own = self.attach_layers(db, attaches);
        layers.lookup_order(&own).cloned().collect()
    }

    /// The file's own layers and the layers it inherits from the files that
    /// source it, flattened into one lookup order.
    fn resolution_layers<'db>(
        self,
        db: &'db dyn Db,
        view: CollationView,
    ) -> Cow<'db, CrossFileLayers> {
        let inherited = self.inherited_layers(db, view);
        if inherited.is_empty() {
            return Cow::Borrowed(self.cross_file_layers(db, view));
        }

        let own = self.cross_file_layers(db, view);
        let mut above = own.above.clone();
        let mut below = own.below.clone();
        for site in inherited {
            above.extend(site.layers.above.iter().cloned());
            below.extend(site.layers.below.iter().cloned());
        }

        // Every alternative's `below` ends in the default search path. Hoist it
        // out and append it once, or the next one's attaches land under `base`.
        let search_path = default_search_path_layers(db);
        below.retain(|layer| !search_path.contains(layer));
        below.extend(search_path);

        Cow::Owned(CrossFileLayers { above, below })
    }

    /// The lookup-ordered layers this file's lazy / end-of-file view sees: the
    /// file's own [`File::cross_file_layers`], then one alternative per file
    /// that sources it (see [`File::inherited_layers`]).
    ///
    /// The alternatives are resolved independently, not as a priority order.
    /// Symbols resolve in each and are returned as a union of results.
    ///
    /// Tracked query, firewall between `resolve()` and the `no_eq`
    /// `semantic_index` read by `attach_layers()`.
    #[salsa::tracked(returns(ref))]
    pub(crate) fn imports_by_sourcing_file(self, db: &dyn Db) -> Vec<Vec<ImportLayer>> {
        let own = self.attach_layers(db, AttachView::Anywhere);
        self.layers_by_sourcing_file(db, CollationView::Lazy)
            .into_iter()
            .map(|layers| layers.lookup_order(&own).cloned().collect())
            .collect()
    }

    /// [`File::imports_by_sourcing_file`], narrowed to `offset` the way
    /// [`File::imports_at`] narrows [`File::imports`].
    ///
    /// Not tracked because keying a cache on `(self, offset)` would add an entry
    /// per cursor position.
    pub(crate) fn imports_by_sourcing_file_at(
        self,
        db: &dyn Db,
        offset: TextSize,
    ) -> Vec<Vec<ImportLayer>> {
        let index = self.semantic_index(db);
        let (cursor_scope, _) = index.scope_at(offset);

        let (collation, attaches) = if index.scope_is_eager(cursor_scope) {
            (
                CollationView::Eager,
                AttachView::Eager(slice::from_ref(&offset)),
            )
        } else {
            (CollationView::Lazy, AttachView::Lazy {
                offset,
                scope_id: cursor_scope,
            })
        };

        let own = self.attach_layers(db, attaches);
        self.layers_by_sourcing_file(db, collation)
            .into_iter()
            .map(|layers| layers.lookup_order(&own).cloned().collect())
            .collect()
    }

    /// The file's own layers, plus one alternative per file that sources it.
    fn layers_by_sourcing_file(self, db: &dyn Db, view: CollationView) -> Vec<&CrossFileLayers> {
        let mut alternatives = vec![self.cross_file_layers(db, view)];
        alternatives.extend(
            self.inherited_layers(db, view)
                .iter()
                .map(|site| &site.layers),
        );
        alternatives
    }

    /// The layers `self` inherits from each file that sources it, one entry per
    /// file in `self.sourced_by(db)`, each recursively including what that file
    /// itself inherits. That recursion is what makes inheritance transitive
    /// across a `main.R -> setup.R -> helpers.R` chain.
    ///
    /// Empty for a file with an explicit load order.
    ///
    /// `cycle_result` is defensive. Resolving a source site reads the target's
    /// `exports`, meaning that a source cycle is always also a `semantic_index`
    /// cycle which has its own recovery.
    #[salsa::tracked(returns(ref), cycle_result =
    inherited_layers_cycle_result)]
    pub(crate) fn inherited_layers(self, db: &dyn Db, view: CollationView) -> Vec<InheritedLayers> {
        if load_context(db, self, view).fixed_load_order {
            return Vec::new();
        }

        self.sourced_by(db)
            .iter()
            .map(|&sourcing_file| build_inherited_layers(db, self, sourcing_file, view))
            .collect()
    }

    /// Bare [`File::imports`], without inheritance.
    pub(crate) fn standalone_imports(self, db: &dyn Db) -> Vec<ImportLayer> {
        let own = self.attach_layers(db, AttachView::Anywhere);
        self.cross_file_layers(db, CollationView::Lazy)
            .lookup_order(&own)
            .cloned()
            .collect()
    }

    /// This file's own `library()` / `require()` attaches as `Package` layers,
    /// in LIFO order (latest-attached first), narrowed to what `view` admits.
    /// Reads the file's own semantic index.
    ///
    /// An attach to a package absent from every root is dropped (no entity).
    fn attach_layers(self, db: &dyn Db, view: AttachView<'_>) -> Vec<ImportLayer> {
        let index = self.semantic_index(db);
        index
            .semantic_calls()
            .iter()
            .rev()
            .filter_map(|call| match call.kind() {
                SemanticCallKind::Attach { package, region } => Some((call, package, region)),
                // A `library()` inside the sourced file is forwarded separately
                // by the semantic index builder as its own `Attach`, scoped to
                // this `source()`.
                SemanticCallKind::Source { .. } => None,
            })
            .filter(|(call, _, region)| view.sees(index, call, region))
            .filter_map(|(_, package, _)| db.package_by_name(package).map(ImportLayer::Package))
            .collect()
    }

    /// The cross-file layers this file sees at load time, excluding its own
    /// attaches (see [`CrossFileLayers`]). Never reads the file's own semantic
    /// index, so it's safe to call while that index is being built.
    ///
    /// Tracked and keyed on `(self, view)`. Resolving one file's effects reads
    /// this once per annotated call, and each rebuild walks every collation
    /// predecessor's `attached_packages`, so recomputing it per call would be
    /// O(predecessors) each time.
    #[salsa::tracked(returns(ref))]
    pub(crate) fn cross_file_layers(self, db: &dyn Db, view: CollationView) -> CrossFileLayers {
        lower_load_context(db, load_context(db, self, view))
    }

    /// The collation members of `self`'s own `R/` directory, in load order.
    ///
    /// Path-based only. The scan-time resolver
    /// ([`SalsaImportsResolver`](crate::imports::SalsaImportsResolver)) calls
    /// `cross_file_layers` while `self`'s own semantic index is still being
    /// built. The query can't recurse into the index.
    #[salsa::tracked(returns(ref))]
    pub(crate) fn collation_siblings(self, db: &dyn Db) -> Vec<File> {
        let Some(dir) = self.path(db).as_path().and_then(Utf8Path::parent) else {
            return Vec::new();
        };
        files_in_directory(db, dir)
    }
}

fn inherited_layers_cycle_result(
    _db: &dyn Db,
    _id: salsa::Id,
    _file: File,
    _view: CollationView,
) -> Vec<InheritedLayers> {
    Vec::new()
}

/// What `sourcing_file` contributes to `target`, its own bands plus what it
/// inherits in turn.
///
/// `sourcing_file`'s own `below` band goes last because the default search
/// path lives at the end of it, and that has to stay at the bottom of the
/// whole chain.
fn build_inherited_layers(
    db: &dyn Db,
    file: File,
    source_site: File,
    view: CollationView,
) -> InheritedLayers {
    let offsets = match view {
        CollationView::Lazy => None,
        CollationView::Eager => source_offsets(db, source_site, file),
    };

    let own_cross = source_site.cross_file_layers(db, view);
    let grandparents = source_site.inherited_layers(db, view);

    let (own_attach, exports_so_far) = match offsets.as_deref() {
        Some(offsets) => (
            source_site.attach_layers(db, AttachView::Eager(offsets)),
            source_site.semantic_index(db).exports_at_sources(offsets),
        ),
        None => (source_site.attach_layers(db, AttachView::Anywhere), None),
    };

    // An unpinned `source()` call may run after any top-level binding, so use
    // the whole file as over-approximation.
    let source_layer = match exports_so_far {
        Some(exports_so_far) => ImportLayer::SourcingFile {
            file: source_site,
            exports_so_far,
        },
        None => ImportLayer::File(source_site),
    };

    // One Source effect can load several files (`sourceDir()`, `tar_source()`).
    // Keep track of already sourced files, since their eager top-level context
    // can see what's already been sourced.
    let mut above: Vec<ImportLayer> = match offsets.as_deref() {
        Some(offsets) => loaded_before(db, source_site, file, offsets)
            .into_iter()
            .map(ImportLayer::File)
            .collect(),
        None => Vec::new(),
    };

    above.push(source_layer);
    above.extend(own_cross.above.iter().cloned());
    above.extend(
        grandparents
            .iter()
            .flat_map(|site| site.layers.above.iter().cloned()),
    );

    let mut below = own_attach;
    below.extend(
        grandparents
            .iter()
            .flat_map(|site| site.layers.below.iter().cloned()),
    );
    below.extend(own_cross.below.iter().cloned());

    InheritedLayers {
        file: source_site,
        layers: CrossFileLayers { above, below },
    }
}

/// Returns source-order offsets for eager `source()` calls from `sourcing_file` to
/// `file`.
///
/// `None` leaves the context unpinned when no calls target `file` or any call is
/// lazy. A lazy call can run after the rest of `sourcing_file`, so no offset bounds
/// its imports.
fn source_offsets(db: &dyn Db, sourcing_file: File, file: File) -> Option<Vec<TextSize>> {
    let index = sourcing_file.semantic_index(db);
    let mut offsets = Vec::new();

    for site in sourcing_file.source_sites(db) {
        if site.target() != Some(file) {
            continue;
        }
        if !index.scope_is_eager(site.scope()) {
            return None;
        }
        offsets.push(site.offset());
    }

    match offsets.is_empty() {
        true => None,
        false => Some(offsets),
    }
}

/// The files the calls at `offsets` loaded before `file`, latest first.
///
/// One Source effect can expand to several targets (`sourceDir()`), which share
/// the call's offset. Only the targets ahead of `file` in its own group have run
/// by the time `file` does.
fn loaded_before(db: &dyn Db, source_file: File, file: File, offsets: &[TextSize]) -> Vec<File> {
    let sites = source_file.source_sites(db);
    let mut loaded: Vec<File> = Vec::new();

    // Descending, so the latest call's targets rank first.
    for &offset in offsets.iter().rev() {
        let targets: Vec<File> = sites
            .iter()
            .filter(|site| site.offset() == offset)
            .filter_map(|site| site.target())
            .collect();
        let Some(own) = targets.iter().position(|target| *target == file) else {
            continue;
        };

        for &target in targets[..own].iter().rev() {
            if !loaded.contains(&target) {
                loaded.push(target);
            }
        }
    }

    loaded
}

/// Lowers a context while preserving resolver precedence. Visible definitions
/// and NAMESPACE imports rank above the file's attaches, and loader-provided
/// search-path layers rank below them.
pub(crate) fn lower_load_context(db: &dyn Db, context: LoadContext) -> CrossFileLayers {
    let LoadContext {
        visible_files,
        namespace_owner,
        implicit_attaches,
        search_path_tail,
        fixed_load_order: _,
    } = context;

    let mut above: Vec<ImportLayer> = visible_files
        .iter()
        .copied()
        .map(ImportLayer::File)
        .collect();
    if let Some(package) = namespace_owner {
        let namespace = package.namespace(db);
        extend_with_namespace_imports(package, namespace, &mut above);
        extend_with_namespace_package_imports(db, namespace, &mut above);
    }

    // `Lazy` includes successor attaches that run later and should outrank this
    // file's own. Ranking them below loses only names shadowed by a package a
    // successor reattaches.
    let mut below = predecessor_attach_layers(db, &visible_files);
    below.extend(
        implicit_attaches
            .iter()
            .filter_map(|name| db.package_by_name(name).map(ImportLayer::Package)),
    );
    match search_path_tail {
        SearchPathTail::Base => below.extend(base_layer(db)),
        SearchPathTail::Default => below.extend(default_search_path_layers(db)),
    }

    CrossFileLayers { above, below }
}

/// The search-path attaches contributed by a set of load-order files, latest
/// file first (the slice is already LIFO), each file's own attaches latest
/// first. Reads each file's `attached_packages`, never the caller's own index.
/// An attach to a package absent from every root is dropped (no entity).
fn predecessor_attach_layers(db: &dyn Db, files: &[File]) -> Vec<ImportLayer> {
    // Warm the indices in forward load order before the LIFO pass below.
    // `files` is LIFO (latest predecessor first). Reading attaches in LIFO
    // order demands the latest predecessor's index first, which recursively
    // demands its own predecessors, and so on. To reduce stack depth, query
    // attached packages in the reverse order so that each file's own
    // predecessors are already built when its turn comes in the LIFO pass.
    for file in files.iter().rev() {
        file.attached_packages(db);
    }

    files
        .iter()
        .flat_map(|file| {
            file.attached_packages(db)
                .iter()
                .rev()
                .filter_map(|name| db.package_by_name(name.text(db)).map(ImportLayer::Package))
        })
        .collect()
}

/// Push the `From` layer if `package`'s namespace has any `importFrom` entries.
fn extend_with_namespace_imports(
    package: Package,
    namespace: &Namespace,
    layers: &mut Vec<ImportLayer>,
) {
    if namespace.imports.is_empty() {
        return;
    }
    layers.push(ImportLayer::From(package));
}

/// Push one `Package` layer per `import(pkg)` directive in the namespace
/// (bulk package imports). Missing packages are silently dropped.
fn extend_with_namespace_package_imports(
    db: &dyn Db,
    namespace: &Namespace,
    layers: &mut Vec<ImportLayer>,
) {
    for pkg_name in &namespace.package_imports {
        if let Some(pkg) = db.package_by_name(pkg_name) {
            layers.push(ImportLayer::Package(pkg));
        }
    }
}

/// `base`, always the last thing R searches. `None` when it isn't scanned into
/// any root (the R system library is normally on `.libPaths()`, so it is).
fn base_layer(db: &dyn Db) -> Option<ImportLayer> {
    db.package_by_name("base").map(ImportLayer::Package)
}

/// The default startup search path as `Package` layers, `stats` first through
/// `base` last. Packages absent from every root drop out.
fn default_search_path_layers(db: &dyn Db) -> Vec<ImportLayer> {
    crate::search::DEFAULT_SEARCH_PATH_PACKAGES
        .iter()
        .filter_map(|name| db.package_by_name(name).map(ImportLayer::Package))
        .collect()
}
