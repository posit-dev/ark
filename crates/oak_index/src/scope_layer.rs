use std::collections::HashMap;

use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use url::Url;

use crate::semantic_index::Directive;
use crate::semantic_index::DirectiveKind;
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
/// one `PackageExports` layer per `library()`/`require()` directive.
///
/// Offsets are discarded since all of a predecessor file's layers are
/// unconditionally visible.
pub fn file_layers(file: Url, index: &SemanticIndex) -> Vec<ScopeLayer> {
    let mut layers = Vec::new();

    // Last definition of each name wins
    let mut exports = HashMap::new();
    for (name, range) in index.file_exports() {
        exports.insert(name.to_string(), range);
    }

    layers.push(ScopeLayer::FileExports { file, exports });
    let dir_layers = directive_layers(index.file_directives());
    layers.extend(dir_layers.into_iter().map(|(_, l)| l));

    layers
}

/// Convert directives into scope-chain layers, each paired with the offset
/// of the directive that produced it.
pub fn directive_layers(directives: &[Directive]) -> Vec<(TextSize, ScopeLayer)> {
    let mut layers = Vec::new();
    for directive in directives {
        let offset = directive.offset();
        match directive.kind() {
            DirectiveKind::Attach(pkg) => {
                layers.push((offset, ScopeLayer::PackageExports(pkg.clone())));
            },
            DirectiveKind::Source(_) => {},
        }
    }
    layers
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
