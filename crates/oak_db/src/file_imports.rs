use std::collections::HashMap;

use biome_rowan::TextSize;
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

/// The default R search path: packages always attached at startup, in
/// the order R's `search()` reports them (last attached = searched
/// first, so `stats` is highest-priority and `base` is lowest).
/// Materialised as `Package` layers; packages absent from the
/// workspace and library roots drop out (the LSP fills them in later
/// via `oak_sources`).
const DEFAULT_SEARCH_PATH: [&str; 7] = [
    "stats",
    "graphics",
    "grDevices",
    "utils",
    "datasets",
    "methods",
    "base",
];

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
            Some(package) => package_imports(self, db, package),
            None => script_imports(self, db),
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
            Some(package) => narrow_package_top_level(self, db, package),
            None => narrow_script_top_level(self, db, offset),
        }
    }
}

fn narrow_script_top_level(file: File, db: &dyn Db, offset: TextSize) -> Vec<ImportLayer> {
    let index = file.semantic_index(db);
    let file_scope = ScopeId::from(0);

    // Keep file-scope `library()` calls that have run by `offset`, in
    // LIFO order (latest-attached first).
    let mut layers: Vec<_> = index
        .semantic_calls()
        .iter()
        .rev()
        .filter(|call| call.scope() == file_scope && call.offset() < offset)
        .filter_map(|call| attach_layer(db, call))
        .collect();

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

fn attach_layer(db: &dyn Db, call: &SemanticCall) -> Option<ImportLayer> {
    match call.kind() {
        SemanticCallKind::Attach { package: name } => {
            db.package_by_name(name).map(ImportLayer::Package)
        },
        SemanticCallKind::Source { .. } => {
            // `source()` injects into local scope, not the search path,
            // so it's not a scope-chain layer.
            None
        },
    }
}

fn script_imports(file: File, db: &dyn Db) -> Vec<ImportLayer> {
    let index = file.semantic_index(db);

    // Reverse: R searches LIFO, so latest-attached comes first.
    let mut layers: Vec<_> = index
        .semantic_calls()
        .iter()
        .rev()
        .filter_map(|call| attach_layer(db, call))
        .collect();

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
    for pkg_name in DEFAULT_SEARCH_PATH {
        if let Some(pkg) = db.package_by_name(pkg_name) {
            layers.push(ImportLayer::Package(pkg));
        }
    }
}
