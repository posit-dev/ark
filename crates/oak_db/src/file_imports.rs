use std::borrow::Cow;
use std::collections::HashMap;

use biome_rowan::TextSize;
use camino::Utf8Path;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::ScopeId;

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
    /// NAMESPACE `importFrom(pkg, name)` entries. Maps each imported
    /// symbol to its source package by name. Translation to `Package`
    /// happens at resolution time.
    From(HashMap<String, String>),
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
pub(crate) struct CrossFileLayers {
    pub above: Vec<ImportLayer>,
    pub below: Vec<ImportLayer>,
}

impl CrossFileLayers {
    /// Flatten to a single lookup-ordered layer list, splicing the file's own
    /// `library()` attaches into the band between the definition/namespace
    /// layers (which outrank them) and the rest of the search path.
    pub(crate) fn splice_own_attaches(self, own: Vec<ImportLayer>) -> Vec<ImportLayer> {
        let CrossFileLayers { mut above, below } = self;
        above.reserve(own.len() + below.len());
        above.extend(own);
        above.extend(below);
        above
    }
}

/// The point in a package's load at which a file views its collation siblings.
#[derive(Clone, Copy)]
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
    /// The import layers visible to this file at end-of-file, in R's lookup
    /// (LIFO) priority order. Symbols that don't have local bindings (are
    /// unbound in the file's semantic index) can be resolved against these
    /// imports.
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
        let own = self.attach_layers(db, None);
        layers.splice_own_attaches(own)
    }

    /// Import layers visible at an `offset` in a file:
    ///
    /// - **Cursor in lazy context**: returns the full lazy view. Lazy
    ///   contexts like functions are treated as if they run after the
    ///   file is fully sourced (over-approximation). Any `library()` /
    ///   collation entry is potentially visible regardless of where it
    ///   appears relative to the cursor.
    ///
    /// - **Top-level cursor (script)**: only `library()` calls that
    ///   have occurred before `offset`. Most recently attached comes
    ///   first.
    ///
    /// - **Top-level cursor (package)**: only collation predecessors
    ///   of this file. Most recently sourced predecessor comes
    ///   first. The package imports and base namespace come last.
    ///
    /// Plain method rather than `#[salsa::tracked]`. Tracking would key the
    /// cache on `(self, offset)`, creating one entry per cursor position.
    /// Skipping the cache is fine because the body just reads already-cached
    /// subqueries (`imports`, `semantic_index`) and applies an O(n) filter.
    pub fn imports_at(self, db: &dyn Db, offset: TextSize) -> Vec<ImportLayer> {
        let index = self.semantic_index(db);
        let file_scope = ScopeId::from(0);
        let (cursor_scope, _) = index.scope_at(offset);

        // Cursor in lazy context. EOF view, same as `imports()`.
        if cursor_scope != file_scope {
            return self.imports(db).clone();
        }

        // Top-level cursor: predecessors only, and own attaches narrowed to the
        // calls that have run by `offset`.
        let layers = self.cross_file_layers(db, CollationView::Eager);
        let own = self.attach_layers(db, Some(offset));
        layers.splice_own_attaches(own)
    }

    /// This file's own `library()` / `require()` attaches as `Package` layers,
    /// in LIFO order (latest-attached first). Reads the file's own semantic
    /// index. `before` selects which calls to include:
    ///
    /// - `None`: every attach. The end-of-file view, used for lazy contexts.
    /// - `Some(offset)`: only top-level (file-scope) calls that have run by
    ///   `offset`. Calls nested in a block (e.g. inside `test_that({})`) are
    ///   dropped, as are calls after the offset.
    ///
    /// An attach to a package absent from every root is dropped (no entity).
    fn attach_layers(self, db: &dyn Db, before: Option<TextSize>) -> Vec<ImportLayer> {
        let index = self.semantic_index(db);
        let file_scope = ScopeId::from(0);
        index
            .semantic_calls()
            .iter()
            .rev()
            .filter(|call| match before {
                Some(offset) => call.scope() == file_scope && call.offset() < offset,
                None => true,
            })
            .filter_map(|call| match call.kind() {
                SemanticCallKind::Attach { package } => {
                    db.package_by_name(package).map(ImportLayer::Package)
                },
                // A `library()` inside the sourced file is forwarded separately
                // by the semantic index builder as its own `Attach`, scoped to
                // this `source()`.
                SemanticCallKind::Source { .. } => None,
            })
            .collect()
    }

    /// The cross-file layers this file sees at load time, excluding its own
    /// attaches (see [`CrossFileLayers`]). Never reads the file's own semantic
    /// index, so it's safe to call while that index is being built.
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
}

fn package_load_layers(
    file: File,
    db: &dyn Db,
    package: Package,
    view: CollationView,
) -> CrossFileLayers {
    let files = package.files(db);

    // The sibling `R/` files visible to this one, in LIFO order (latest-sourced
    // first): a name defined late in the collation shadows the same name defined
    // earlier. Self is excluded, its own top-level bindings come from `exports`,
    // and including it here would cycle in `resolve` for unbound names.
    let def_files: Vec<File> = match view {
        CollationView::Lazy => files.iter().rev().copied().filter(|f| *f != file).collect(),
        CollationView::Eager => match files.iter().position(|f| *f == file) {
            Some(pos) => files[..pos].iter().rev().copied().collect(),
            None => {
                // File claims membership but isn't in the package's `files`.
                // Shouldn't happen.
                log::warn!(
                    "File {file} has package back-pointer to {package} but is not in its files",
                    file = file.path(db),
                    package = package.name(db),
                );
                files.iter().rev().copied().filter(|f| *f != file).collect()
            },
        },
    };

    let mut above: Vec<ImportLayer> = def_files.iter().copied().map(ImportLayer::File).collect();
    let namespace = package.namespace(db);
    extend_with_namespace_imports(namespace, &mut above);
    extend_with_namespace_package_imports(db, namespace, &mut above);

    // Every def file's attaches go on the search path below the file's own.
    // For the `Lazy` view that includes successors, whose `library()` calls
    // actually run after this file's at load time and so outrank the file's own
    // attaches at runtime. We rank them below instead. Only matters when a
    // successor re-attaches a package that shadows one of this file's own
    // attaches, which is rare, and the direction we lose is the safe one.
    let mut below = predecessor_attach_layers(db, &def_files);
    below.extend(base_layer(db));
    CrossFileLayers { above, below }
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
    extend_with_namespace_imports(namespace, &mut above);
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

/// Push the `From` layer if the namespace has any `importFrom` entries.
/// Collects them into a single map from name to source package.
fn extend_with_namespace_imports(namespace: &Namespace, layers: &mut Vec<ImportLayer>) {
    if namespace.imports.is_empty() {
        return;
    }
    let map: HashMap<String, String> = namespace
        .imports
        .iter()
        .map(|imp| (imp.name.clone(), imp.package.clone()))
        .collect();
    layers.push(ImportLayer::From(map));
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
