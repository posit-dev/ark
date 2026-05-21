use std::collections::HashMap;

use oak_semantic::semantic_index::SemanticCallKind;

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
    PackageImports(HashMap<String, String>),
    /// An attached package's exports. Missing packages are filtered
    /// out by `imports`.
    PackageExports(Package),
}

/// The default R search path: packages always attached at startup, in
/// the order R's `search()` reports them (last attached = searched
/// first, so `stats` is highest-priority and `base` is lowest).
/// Materialised as `PackageExports` layers; packages absent from the
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
    /// The import layers that exist for this file, in priority order.
    ///
    /// Offset-independent and stable across cursor moves. Recomputed
    /// only when the file's package membership, NAMESPACE, or this
    /// file's semantic calls actually change.
    #[salsa::tracked(returns(ref))]
    pub fn imports(self, db: &dyn Db) -> Vec<ImportLayer> {
        match self.package(db) {
            Some(package) => package_imports(self, db, package),
            None => script_imports(self, db),
        }
    }
}

fn script_imports(file: File, db: &dyn Db) -> Vec<ImportLayer> {
    let mut layers = Vec::new();
    let index = file.semantic_index(db);

    // `library()` / `require()` calls in source order. We keep all of
    // them; callers that need offset-based narrowing filter at query
    // time.
    for call in index.semantic_calls() {
        match call.kind() {
            SemanticCallKind::Attach { package: name } => {
                if let Some(package) = db.package_by_name(name) {
                    layers.push(ImportLayer::PackageExports(package));
                }
            },
            SemanticCallKind::Source { .. } => {
                // `source()` injects into local scope, not the search path,
                // so it's not a scope-chain layer.
            },
        }
    }

    // Default search path (R always attaches these at startup).
    extend_with_default_search_path(db, &mut layers);

    layers
}

fn package_imports(_file: File, db: &dyn Db, package: Package) -> Vec<ImportLayer> {
    let mut layers = Vec::new();
    let namespace = package.namespace(db);

    // NAMESPACE `importFrom(pkg, name)` directives. Collect them into
    // a single layer that maps each imported symbol name to its source
    // package.
    if !namespace.imports.is_empty() {
        let map: HashMap<String, String> = namespace
            .imports
            .iter()
            .map(|imp| (imp.name.clone(), imp.package.clone()))
            .collect();
        layers.push(ImportLayer::PackageImports(map));
    }

    // NAMESPACE `import(pkg)` directives (bulk package imports).
    for pkg_name in &namespace.package_imports {
        if let Some(pkg) = db.package_by_name(pkg_name) {
            layers.push(ImportLayer::PackageExports(pkg));
        }
    }

    // All files in the package, in `Package.files` order. Query-time
    // narrowing filters to predecessors of a given file (R's collation
    // order) when only those are visible.
    //
    // TODO(scan): once `oak_scan` orders `Package.files` by the
    // DESCRIPTION `Collate` field, this iteration becomes
    // collation-ordered directly. Today `Package.files` is whatever
    // order the scanner produces.
    for &pkg_file in package.files(db) {
        layers.push(ImportLayer::File(pkg_file));
    }

    // `base` is always implicitly available inside a package.
    if let Some(base) = db.package_by_name("base") {
        layers.push(ImportLayer::PackageExports(base));
    }

    layers
}

fn extend_with_default_search_path(db: &dyn Db, layers: &mut Vec<ImportLayer>) {
    for pkg_name in DEFAULT_SEARCH_PATH {
        if let Some(pkg) = db.package_by_name(pkg_name) {
            layers.push(ImportLayer::PackageExports(pkg));
        }
    }
}
