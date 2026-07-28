use std::borrow::Cow;

use biome_rowan::TextSize;
use camino::Utf8Path;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::semantic_index::AttachRegion;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticCall;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticIndex;

use crate::Db;
use crate::File;
use crate::Package;

/// A layer in a file's import chain.
///
/// Carries salsa entity ids (`File`, `Package`) end-to-end. No URL or
/// package-name strings cross out of `oak_db` for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportLayer {
    /// A predecessor file in a package's collation, or another workspace
    /// file. Names are resolved through `file.exports(db)`.
    File(File),
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
/// attaches, the test runner's implicit attaches, and `base`.
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
enum AttachView {
    /// Every attach in the file.
    ///
    /// Over-approximates on two axes. An attach in a function body counts even
    /// though the body may never run, and a conditional one counts even though
    /// its branch may not have been taken.
    Anywhere,
    /// Attaches visible at `offset` in lazy `scope`.
    ///
    /// An attach in `scope_id` or an enclosing lazy body applies only after its
    /// `library()` call. The lazy view treats an unconditional top-level attach
    /// as visible regardless of position, which over-approximates this case:
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
    /// The attaches that have run and still hold at `offset` in an eagerly
    /// evaluated scope. Calls reached only by running a lazy body are dropped,
    /// as are calls after the offset.
    Eager(TextSize),
}

impl AttachView {
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
            AttachView::Eager(offset) => {
                index.scope_is_eager(call.scope()) && region.contains(call, offset)
            },
        }
    }
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
        let layers = self.cross_file_layers(db, CollationView::Lazy);
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
            (CollationView::Eager, AttachView::Eager(offset))
        } else {
            (CollationView::Lazy, AttachView::Lazy {
                offset,
                scope_id: cursor_scope,
            })
        };

        let layers = self.cross_file_layers(db, collation);
        let own = self.attach_layers(db, attaches);
        layers.lookup_order(&own).cloned().collect()
    }

    /// This file's own `library()` / `require()` attaches as `Package` layers,
    /// in LIFO order (latest-attached first), narrowed to what `view` admits.
    /// Reads the file's own semantic index.
    ///
    /// An attach to a package absent from every root is dropped (no entity).
    fn attach_layers(self, db: &dyn Db, view: AttachView) -> Vec<ImportLayer> {
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
        match self.package(db) {
            // A `tests/testthat/` file: sees the whole package plus sourced
            // helpers, with testthat attached.
            Some(package) if is_testthat_file(self, db) => testthat_load_layers(self, db, package),
            // A loadable `R/` file: sees collation siblings and the package
            // NAMESPACE.
            Some(package) if self.is_package_source(db, package) => {
                package_load_layers(self, db, package, view)
            },
            // A non-package script in an `R/` directory: collated
            // alphabetically, exactly like a package `R/` with no `Collate:`.
            None if in_r_directory(self, db) => script_collation_layers(self, db, view),
            // A standalone script, or a file with a package back-pointer that
            // isn't a loadable `R/` file (`data-raw/`, `inst/`, a non-collated
            // `R/` file): lives in the package but isn't loaded with it, so it
            // sees only its own attaches and the default search path.
            _ => CrossFileLayers {
                above: Vec::new(),
                below: default_search_path_layers(db),
            },
        }
    }

    /// Whether this file is one of `package`'s loadable `R/` files, the ones in
    /// `package.files()`. A file can carry a package back-pointer without being
    /// loadable: `data-raw/`, `inst/`, and `R/` files left out of a `Collate:`
    /// directive all land in `package.scripts()` instead and resolve as
    /// standalone scripts.
    fn is_package_source(self, db: &dyn Db, package: Package) -> bool {
        package.files(db).contains(&self)
    }

    /// The collation members of `self`'s own `R/` directory, in load order:
    /// sorted by basename, ASCII case-insensitively, the same order a
    /// package `R/` with no `Collate:` gets from
    /// `oak_scan::packages::order_alphabetically`.
    ///
    /// Path-based only. The scan-time resolver
    /// ([`SalsaImportsResolver`](crate::imports::SalsaImportsResolver)) calls
    /// `cross_file_layers` while `self`'s own semantic index is still being
    /// built. The query can't recurse into the index.
    ///
    /// Gathers candidates from workspace roots only (`root.scripts(db)` for
    /// each). `OrphanRoot`, library roots, and `StaleRoot` don't contribute
    /// collation siblings.
    #[salsa::tracked(returns(ref))]
    pub(crate) fn collation_siblings(self, db: &dyn Db) -> Vec<File> {
        let Some(dir) = self.path(db).as_path().and_then(Utf8Path::parent) else {
            return Vec::new();
        };

        let mut siblings: Vec<File> = db
            .workspace_roots()
            .roots(db)
            .iter()
            .flat_map(|root| root.scripts(db).iter().copied())
            .filter(|file| file.path(db).as_path().and_then(Utf8Path::parent) == Some(dir))
            .collect();

        siblings.sort_by_cached_key(|file| collation_basename_key(*file, db));
        siblings
    }
}

fn package_load_layers(
    file: File,
    db: &dyn Db,
    package: Package,
    view: CollationView,
) -> CrossFileLayers {
    let files = package.files(db);

    // `Collate:` order isn't derivable from file names.
    let prefix_len = files.iter().position(|sibling| *sibling == file);
    if prefix_len.is_none() && matches!(view, CollationView::Eager) {
        // File claims package membership but isn't in `package.files()`.
        // Shouldn't happen; see the placement invariant on `File.package`.
        log::warn!(
            "File {file} has package back-pointer to {package} but is not in its files",
            file = file.path(db),
            package = package.name(db),
        );
    }
    let siblings = visible_siblings(file, files, view, prefix_len);

    let mut above: Vec<ImportLayer> = siblings.iter().copied().map(ImportLayer::File).collect();
    let namespace = package.namespace(db);
    extend_with_namespace_imports(package, namespace, &mut above);
    extend_with_namespace_package_imports(db, namespace, &mut above);

    // Every sibling's attaches go on the search path below the file's own.
    // For the `Lazy` view that includes successors, whose `library()` calls
    // actually run after this file's at load time and so outrank the file's own
    // attaches at runtime. We rank them below instead. Only matters when a
    // successor re-attaches a package that shadows one of this file's own
    // attaches, which is rare, and the direction we lose is the safe one.
    let mut below = predecessor_attach_layers(db, &siblings);
    below.extend(base_layer(db));
    CrossFileLayers { above, below }
}

/// Load-time layers for a non-package script collated by the `R/` directory
/// convention (see `File::cross_file_layers`). Mirrors `package_load_layers`,
/// with `below` ending in the whole default search path rather than just `base`.
fn script_collation_layers(file: File, db: &dyn Db, view: CollationView) -> CrossFileLayers {
    let files = file.collation_siblings(db);

    // `file` is missing from its own sibling list until the scanner moves it out
    // of `OrphanRoot`. Cut on the sort key instead.
    let own_key = collation_basename_key(file, db);
    let prefix_len =
        files.partition_point(|sibling| collation_basename_key(*sibling, db) < own_key);
    let siblings = visible_siblings(file, files, view, Some(prefix_len));

    let above: Vec<ImportLayer> = siblings.iter().copied().map(ImportLayer::File).collect();

    let mut below = predecessor_attach_layers(db, &siblings);
    below.extend(default_search_path_layers(db));
    CrossFileLayers { above, below }
}

/// The sibling files visible to `file`, in LIFO order (latest-sourced first):
/// a name defined late in the collation shadows the same name defined earlier.
/// Self is excluded, its own top-level bindings come from `exports`, and
/// including it here would cycle in `resolve` for unbound names.
///
/// `prefix_len` is how many of `collation` load before `file`, which is all the
/// `Eager` view keeps. `None` means the caller couldn't place `file` in the
/// collation at all, so it falls back to the full LIFO view, over-approximating
/// in the safe direction.
fn visible_siblings(
    file: File,
    collation: &[File],
    view: CollationView,
    prefix_len: Option<usize>,
) -> Vec<File> {
    match view {
        CollationView::Lazy => collation
            .iter()
            .rev()
            .copied()
            .filter(|sibling| *sibling != file)
            .collect(),
        CollationView::Eager => match prefix_len {
            Some(len) => collation[..len].iter().rev().copied().collect(),
            None => collation
                .iter()
                .rev()
                .copied()
                .filter(|sibling| *sibling != file)
                .collect(),
        },
    }
}

/// Load-time layers visible to a `tests/testthat/` file, in R's LIFO priority
/// order.
///
/// A test file runs with the package loaded and `testthat` attached, after
/// testthat has sourced the package's `helper*.R` and `setup*.R` files into
/// the test environment. So the layering, highest priority first, is:
///
/// 1. helper/setup files (sourced into the test env, shadow everything),
/// 2. the whole package's `R/` code,
/// 3. the package's NAMESPACE imports,
/// 4. the file's own top-level `library()` calls (spliced in by the caller),
/// 5. helper/setup and package attaches, then `testthat`, on the search path,
/// 6. base.
fn testthat_load_layers(file: File, db: &dyn Db, package: Package) -> CrossFileLayers {
    // testthat sources `helper*.R` / `setup*.R` sorted, so reversing gives LIFO
    // precedence. Self is dropped when the file being analysed is itself a
    // helper/setup file, same self-exclusion reasoning as `package_load_layers`.
    let mut support: Vec<File> = package
        .scripts(db)
        .iter()
        .copied()
        .filter(|f| *f != file && is_testthat_support_file(*f, db))
        .collect();
    support.sort_by_cached_key(|f| testthat_support_key(*f, db));
    support.reverse();

    // The whole package is loaded when tests run, so every `R/` file is visible.
    // Collation order reversed for LIFO, same as `package_load_layers`.
    let package_files: Vec<File> = package.files(db).iter().rev().copied().collect();

    let mut above: Vec<ImportLayer> = support
        .iter()
        .chain(package_files.iter())
        .copied()
        .map(ImportLayer::File)
        .collect();
    let namespace = package.namespace(db);
    extend_with_namespace_imports(package, namespace, &mut above);
    extend_with_namespace_package_imports(db, namespace, &mut above);

    // Attaches from the sourced helpers and the loaded package, then testthat
    // (attached first by the runner, so lowest), then base. The test file's own
    // attaches are spliced above these by the caller.
    let mut below = predecessor_attach_layers(db, &support);
    below.extend(predecessor_attach_layers(db, &package_files));
    below.extend(db.package_by_name("testthat").map(ImportLayer::Package));
    below.extend(base_layer(db));

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

/// True when `file` sits directly in a `tests/testthat/` directory, the
/// layout testthat sources and runs files from. This is what separates a
/// test file from an ordinary package script under e.g. `tests/` or `inst/`.
fn is_testthat_file(file: File, db: &dyn Db) -> bool {
    match file.path(db).as_file() {
        Some(path) => in_testthat_dir(path.as_path()),
        None => false,
    }
}

fn in_testthat_dir(path: &Utf8Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    parent.file_name() == Some("testthat") &&
        parent.parent().and_then(Utf8Path::file_name) == Some("tests")
}

/// True when `file` sits directly in an `R/` directory, the convention that
/// triggers script collation for a non-package file (see the match in
/// `File::cross_file_layers`). Case-sensitive: that's the convention on every
/// platform, and what the package scanner looks for.
fn in_r_directory(file: File, db: &dyn Db) -> bool {
    let Some(path) = file.path(db).as_path() else {
        return false;
    };
    path.parent().and_then(Utf8Path::file_name) == Some("R")
}

/// testthat sources `helper*.R` and `setup*.R` from `tests/testthat/` into the
/// test environment before running any test file, so their top-level bindings
/// are visible to every test. testthat matches `^helper.*\.[rR]$` and
/// `^setup.*\.[rR]$`; only the basename prefix matters here, since
/// `package.scripts` already holds nothing but `.R` files. Teardown files are
/// sourced after tests and rarely define names tests reference, so they're left
/// out.
fn is_testthat_support_file(file: File, db: &dyn Db) -> bool {
    if !is_testthat_file(file, db) {
        return false;
    }
    match file.path(db).file_name() {
        Some(name) => name.starts_with("helper") || name.starts_with("setup"),
        None => false,
    }
}

/// Sort key for support files, matching testthat's `sort(dir(...))` order.
/// We sort by raw basename (byte order = C locale for ASCII): case-sensitive
/// like testthat, and platform-stable. This is a bit different to testthat
/// which currently sorts based on locale, but arguably this should be fixed on
/// the testthat side.
fn testthat_support_key(file: File, db: &dyn Db) -> Cow<'_, str> {
    file.path(db).file_name().unwrap_or_default()
}

/// Case-insensitive basename sort key for `collation_siblings`, matching
/// `oak_scan::packages::order_alphabetically`'s `basename_key` so a
/// non-package `R/` collates the same way as a package `R/` with no
/// `Collate:`.
fn collation_basename_key(file: File, db: &dyn Db) -> Option<String> {
    file.path(db)
        .file_name()
        .map(|name| name.to_ascii_lowercase())
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
