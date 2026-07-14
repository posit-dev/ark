use std::borrow::Cow;
use std::collections::HashMap;

use biome_rowan::TextSize;
use camino::Utf8Path;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::semantic_index::SemanticCall;
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
        match self.package(db) {
            Some(package) if is_testthat_file(self, db) => {
                testthat_imports(self, db, package, None)
            },
            Some(package) if is_package_source(self, db, package) => {
                package_imports(self, db, package)
            },
            // A file with a package back-pointer that isn't a loadable `R/`
            // file (`data-raw/`, `inst/`, a non-collated `R/` file) lives in
            // the package but isn't loaded with it. Resolve it as a standalone
            // script, same as a file with no package at all.
            _ => script_imports(self, db),
        }
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

        // Top-level cursor: sequential narrowing.
        match self.package(db) {
            // Helpers, setup, the package, and testthat are all sourced or
            // attached before a test file's body runs, so they stay visible
            // at any offset. Only the file's own top-level `library()` calls
            // narrow.
            Some(package) if is_testthat_file(self, db) => {
                testthat_imports(self, db, package, Some(offset))
            },
            Some(package) if is_package_source(self, db, package) => {
                narrow_package_top_level(self, db, package)
            },
            // A packaged file that isn't a loadable `R/` file narrows like a
            // standalone script.
            _ => narrow_script_top_level(self, db, offset),
        }
    }
}

/// Whether `file` is one of its package's loadable `R/` files, the ones in
/// `package.files()`. A file can carry a package back-pointer without being
/// loadable: `data-raw/`, `inst/`, and `R/` files left out of a `Collate:`
/// directive all land in `package.scripts()` instead and resolve as
/// standalone scripts.
fn is_package_source(file: File, db: &dyn Db, package: Package) -> bool {
    package.files(db).contains(&file)
}

fn narrow_script_top_level(file: File, db: &dyn Db, offset: TextSize) -> Vec<ImportLayer> {
    let mut layers = attach_layers(file, db, Some(offset));
    extend_with_default_search_path(db, &mut layers);
    layers
}

fn narrow_package_top_level(file: File, db: &dyn Db, package: Package) -> Vec<ImportLayer> {
    let files = package.files(db);
    let Some(file_pos) = files.iter().position(|f| *f == file) else {
        // File claims membership but isn't in the package's `files`.
        // Shouldn't happen.
        log::warn!(
            "File {file} has package back-pointer to {package} but is not in its files",
            file = file.path(db),
            package = package.name(db),
        );
        return file.imports(db).clone();
    };

    let mut layers = Vec::new();

    // Predecessors only, in LIFO order (latest-sourced first).
    layers.extend(
        files[..file_pos]
            .iter()
            .rev()
            .copied()
            .map(ImportLayer::File),
    );

    let namespace = package.namespace(db);
    extend_with_namespace_imports(namespace, &mut layers);
    extend_with_namespace_package_imports(db, namespace, &mut layers);

    extend_with_base(db, &mut layers);
    layers
}

/// The file's `library()` / `require()` attaches as `Package` layers, in LIFO
/// order (latest-attached first). `before` selects which calls to include:
///
/// - `None`: every attach. The end-of-file view, used for lazy contexts.
/// - `Some(offset)`: only top-level (file-scope) calls that have run by
///   `offset`. Calls nested in a block (e.g. inside `test_that({})`) are
///   dropped, as are calls after the offset.
fn attach_layers(file: File, db: &dyn Db, before: Option<TextSize>) -> Vec<ImportLayer> {
    let index = file.semantic_index(db);
    let file_scope = ScopeId::from(0);
    index
        .semantic_calls()
        .iter()
        .rev()
        .filter(|call| match before {
            Some(offset) => call.scope() == file_scope && call.offset() < offset,
            None => true,
        })
        .filter_map(|call| attach_layer(db, call))
        .collect()
}

fn attach_layer(db: &dyn Db, call: &SemanticCall) -> Option<ImportLayer> {
    match call.kind() {
        SemanticCallKind::Attach { package: name } => {
            db.package_by_name(name).map(ImportLayer::Package)
        },
        SemanticCallKind::Source { .. } => {
            // A `library()` inside the sourced file is forwarded separately by
            // the semantic index builder as its own `Attach`, scoped to this
            // `source()`.
            None
        },
    }
}

fn script_imports(file: File, db: &dyn Db) -> Vec<ImportLayer> {
    let mut layers = attach_layers(file, db, None);
    extend_with_default_search_path(db, &mut layers);
    layers
}

fn package_imports(file: File, db: &dyn Db, package: Package) -> Vec<ImportLayer> {
    let mut layers = Vec::new();

    // All package files except self, in LIFO order (latest-sourced first).
    // Self is excluded: a file's own top-level bindings come from `exports`,
    // and including self here would create a cycle in `resolve` for unbound
    // names.
    //
    // `package.files(db)` is collation-ordered (see `Package::files`), so
    // reversing it gives R's LIFO precedence: a name defined late in the
    // collation shadows the same name defined earlier.
    layers.extend(
        package
            .files(db)
            .iter()
            .rev()
            .copied()
            .filter(|f| *f != file)
            .map(ImportLayer::File),
    );

    let namespace = package.namespace(db);
    extend_with_namespace_imports(namespace, &mut layers);
    extend_with_namespace_package_imports(db, namespace, &mut layers);

    extend_with_base(db, &mut layers);
    layers
}

/// Imports visible to a `tests/testthat/` file, in R's LIFO priority order.
///
/// A test file runs with the package loaded and `testthat` attached, after
/// testthat has sourced the package's `helper*.R` and `setup*.R` files into
/// the test environment. So the layering, highest priority first, is:
///
/// 1. helper/setup files (sourced into the test env, shadow everything),
/// 2. the whole package's `R/` code,
/// 3. the package's NAMESPACE imports,
/// 4. the file's own top-level `library()` calls,
/// 5. `testthat` on the search path,
/// 6. base.
///
/// `offset` narrows the components of layer 4. `None` produces the end-of-file
/// view and uses every `library()` call. `Some(offset)` uses only the calls
/// that have run by `offset`. The other layers are sourced or attached before
/// the file body runs, so they never narrow.
fn testthat_imports(
    file: File,
    db: &dyn Db,
    package: Package,
    offset: Option<TextSize>,
) -> Vec<ImportLayer> {
    let mut layers = Vec::new();

    // testthat sources `helper*.R` / `setup*.R` sorted, so reversing gives
    // LIFO precedence. Self is dropped when the file being
    // analysed is itself a helper/setup file: its own bindings come from
    // `exports`, and keeping it would create a cycle in `resolve()` for
    // unbound names (same reasoning as self-exclusion in `package_imports()`).
    let mut support: Vec<File> = package
        .scripts(db)
        .iter()
        .copied()
        .filter(|f| *f != file && is_testthat_support_file(*f, db))
        .collect();
    support.sort_by_cached_key(|f| testthat_support_key(*f, db));
    layers.extend(support.into_iter().rev().map(ImportLayer::File));

    // The whole package is loaded when tests run, so every `R/` file is
    // visible. Collation order reversed for LIFO, same as `package_imports()`.
    layers.extend(
        package
            .files(db)
            .iter()
            .rev()
            .copied()
            .map(ImportLayer::File),
    );

    let namespace = package.namespace(db);
    extend_with_namespace_imports(namespace, &mut layers);
    extend_with_namespace_package_imports(db, namespace, &mut layers);

    // The test file's own top-level `library()` / `require()` calls attach to
    // the search path, below the package namespace and its imports but above
    // `testthat` (they run after the runner attached it).
    layers.extend(attach_layers(file, db, offset));

    if let Some(testthat) = db.package_by_name("testthat") {
        layers.push(ImportLayer::Package(testthat));
    }

    extend_with_base(db, &mut layers);
    layers
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

/// Push the `base` package as a `Package` layer. `base` is always
/// implicitly available inside a package.
fn extend_with_base(db: &dyn Db, layers: &mut Vec<ImportLayer>) {
    if let Some(base) = db.package_by_name("base") {
        layers.push(ImportLayer::Package(base));
    }
}

fn extend_with_default_search_path(db: &dyn Db, layers: &mut Vec<ImportLayer>) {
    for pkg_name in crate::search::DEFAULT_SEARCH_PATH_PACKAGES {
        if let Some(pkg) = db.package_by_name(pkg_name) {
            layers.push(ImportLayer::Package(pkg));
        }
    }
}
