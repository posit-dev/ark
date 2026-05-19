use std::collections::HashMap;

use biome_rowan::TextRange;
use oak_package_metadata::namespace::Namespace;
use url::Url;

use crate::semantic_index::SemanticCallKind;
use crate::semantic_index::SemanticIndex;

/// A layer in the scope chain. Layers are ordered most-local-first; resolution
/// iterates front-to-back, first match wins.
#[derive(Debug, Clone)]
pub enum ScopeLayer {
    /// Bindings from a project file's top-level definitions.
    /// When a name is defined multiple times, the last definition wins.
    FileExports {
        file: Url,
        exports: HashMap<String, TextRange>,
    },

    /// Imports from e.g. `importFrom`. Maps symbol name to package name.
    PackageImports(HashMap<String, String>),

    /// Exports of an attached package (`library()` or NAMESPACE `import()`).
    PackageExports(String),
}

/// Compute the scope layers that a single file contributes to the
/// scope chain: one `FileExports` layer from its top-level definitions, plus
/// one `PackageExports` layer per `library()`/`require()` semantic call.
pub fn file_layers(file: Url, index: &SemanticIndex) -> Vec<ScopeLayer> {
    let mut layers = Vec::new();

    let exports = index
        .file_exports()
        .into_iter()
        .map(|(name, range)| (name.to_string(), range))
        .collect();

    layers.push(ScopeLayer::FileExports { file, exports });

    for call in index.semantic_calls() {
        match call.kind() {
            SemanticCallKind::Attach { package } => {
                layers.push(ScopeLayer::PackageExports(package.clone()));
            },
            SemanticCallKind::Source { .. } => {
                // `source()` injects into local scope, not the search path;
                // not a scope-chain layer.
            },
        }
    }

    layers
}

/// The default R search path for scripts: the default packages that R
/// attaches on startup, in search order (last attached = searched first).
pub fn default_search_path() -> Vec<ScopeLayer> {
    // R's default packages, in reverse attachment order (most recently
    // attached first). These are always on the search path unless
    // overridden by `R_DEFAULT_PACKAGES`.
    let default_packages = [
        "utils",
        "stats",
        "datasets",
        "methods",
        "grDevices",
        "graphics",
        "base",
    ];
    default_packages
        .into_iter()
        .map(|pkg| ScopeLayer::PackageExports(pkg.to_string()))
        .collect()
}

/// Build the root layers for a package from its NAMESPACE.
///
/// These go at the back of every file's scope chain:
/// - `PackageImports` from `importFrom()` directives (name → package)
/// - `PackageExports` from `import()` directives
/// - `PackageExports` for `base` (always implicitly available)
pub fn package_root_layers(namespace: &Namespace) -> Vec<ScopeLayer> {
    let mut layers = Vec::new();

    if !namespace.imports.is_empty() {
        let map = namespace
            .imports
            .iter()
            .map(|imp| (imp.name.clone(), imp.package.clone()))
            .collect();
        layers.push(ScopeLayer::PackageImports(map));
    }

    for pkg in &namespace.package_imports {
        layers.push(ScopeLayer::PackageExports(pkg.clone()));
    }

    layers.push(ScopeLayer::PackageExports("base".to_string()));

    layers
}
