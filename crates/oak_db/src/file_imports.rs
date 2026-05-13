use std::collections::HashMap;

use oak_semantic::semantic_index::SemanticCallKind;

use crate::Db;
use crate::File;
use crate::Name;
use crate::Package;
use crate::SourceNode;

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
/// search order (last attached = searched first). Materialised as
/// `PackageExports` layers; packages absent from the source graph
/// drop out (the LSP fills them in later via `oak_sources`).
const DEFAULT_SEARCH_PATH: [&str; 7] = [
    "utils",
    "stats",
    "datasets",
    "methods",
    "grDevices",
    "graphics",
    "base",
];

#[salsa::tracked]
impl File {
    /// The import layers that exist for this file, in priority order.
    ///
    /// Offset-independent and stable across cursor moves. Recomputed
    /// only when the source graph, NAMESPACE, or this file's semantic
    /// calls actually change.
    #[salsa::tracked(returns(ref))]
    pub fn imports(self, db: &dyn Db) -> Vec<ImportLayer> {
        match self.parent(db) {
            Some(SourceNode::Package(package)) => package_layers(self, db, package),
            _ => script_layers(self, db),
        }
    }
}

fn script_layers(file: File, db: &dyn Db) -> Vec<ImportLayer> {
    let mut layers = Vec::new();
    let index = file.semantic_index(db);

    // `library()` / `require()` calls in source order. We keep all of
    // them; callers that need offset-based narrowing filter at query
    // time.
    for call in index.semantic_calls() {
        let SemanticCallKind::Attach { package: name } = call.kind() else {
            continue;
        };
        if let Some(package) = package_by_name(db, name) {
            layers.push(ImportLayer::PackageExports(package));
        }
    }

    // Default search path (R always attaches these at startup).
    extend_with_default_search_path(db, &mut layers);

    layers
}

fn package_layers(_file: File, db: &dyn Db, package: Package) -> Vec<ImportLayer> {
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
        if let Some(pkg) = package_by_name(db, pkg_name) {
            layers.push(ImportLayer::PackageExports(pkg));
        }
    }

    // All collation files in declaration order. These can be narrowed
    // at query time to match a particular R file scope where only its
    // predecessors in the collation order are visible.
    for collation_file in package.collation(db) {
        layers.push(ImportLayer::File(*collation_file));
    }

    // `base` is always implicitly available inside a package.
    if let Some(base) = package_by_name(db, "base") {
        layers.push(ImportLayer::PackageExports(base));
    }

    layers
}

fn extend_with_default_search_path(db: &dyn Db, layers: &mut Vec<ImportLayer>) {
    for pkg_name in DEFAULT_SEARCH_PATH {
        if let Some(pkg) = package_by_name(db, pkg_name) {
            layers.push(ImportLayer::PackageExports(pkg));
        }
    }
}

fn package_by_name(db: &dyn Db, name: &str) -> Option<Package> {
    db.source_graph()
        .package_by_name(db, Name::new(db, name))
}
