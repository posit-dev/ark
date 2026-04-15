use std::collections::HashMap;

use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_package::library::Library;
use oak_package::package_namespace::Namespace;
use url::Url;

use crate::semantic_index::Directive;
use crate::semantic_index::DirectiveKind;
use crate::semantic_index::SemanticIndex;

/// A layer in the scope chain. Layers are ordered most-local-first; resolution
/// iterates front-to-back, first match wins.
#[derive(Debug, Clone)]
pub enum BindingSource {
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

/// The result of resolving a name against the external scope chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalDefinition {
    /// Defined in a project file.
    ProjectFile {
        file: Url,
        name: String,
        range: TextRange,
    },

    /// Found in an installed package (via `importFrom()`, `library()`, etc.).
    Package { package: String, name: String },
}

/// Walk the scope chain front-to-back, returning the first match.
pub fn resolve_external_name(
    library: &Library,
    scope: &[BindingSource],
    name: &str,
) -> Option<ExternalDefinition> {
    for source in scope {
        match source {
            BindingSource::FileExports { file, exports } => {
                if let Some(range) = exports.get(name) {
                    return Some(ExternalDefinition::ProjectFile {
                        file: file.clone(),
                        name: name.to_string(),
                        range: *range,
                    });
                }
            },

            BindingSource::PackageImports(names) => {
                if let Some(pkg) = names.get(name) {
                    return Some(ExternalDefinition::Package {
                        package: pkg.clone(),
                        name: name.to_string(),
                    });
                }
            },

            BindingSource::PackageExports(pkg_name) => {
                let Some(pkg) = library.get(pkg_name) else {
                    continue;
                };
                if pkg.exported_symbols.contains_str(name) {
                    return Some(ExternalDefinition::Package {
                        package: pkg_name.clone(),
                        name: name.to_string(),
                    });
                }
            },
        }
    }

    None
}

/// Resolve a name in a specific package's exported symbols.
pub fn resolve_in_package(
    library: &Library,
    package: &str,
    name: &str,
) -> Option<ExternalDefinition> {
    let pkg = library.get(package)?;
    if pkg
        .exported_symbols
        .binary_search(&name.to_string())
        .is_ok()
    {
        return Some(ExternalDefinition::Package {
            package: package.to_string(),
            name: name.to_string(),
        });
    }
    None
}

/// Compute the binding-source layers that a single file contributes to the
/// scope chain: one `FileExports` layer from its top-level definitions, plus
/// one `PackageExports` layer per `library()`/`require()` directive.
///
/// `Source` directives are skipped here because resolving them requires the
/// sourced file's index. Use [`directive_layers`] with a resolver callback
/// when cross-file resolution is available. Offsets are discarded since
/// all of a predecessor file's layers are unconditionally visible.
pub fn file_layers(file: Url, index: &SemanticIndex) -> Vec<BindingSource> {
    let mut layers = Vec::new();

    // Last definition of each name wins
    let mut exports = HashMap::new();
    for (name, range) in index.file_exports() {
        exports.insert(name.to_string(), range);
    }

    layers.push(BindingSource::FileExports { file, exports });
    let dir_layers = directive_layers(index.file_directives(), |_| None);
    layers.extend(dir_layers.into_iter().map(|(_, l)| l));

    layers
}

/// Convert directives into scope-chain layers, each paired with the offset
/// of the directive that produced it.
///
/// `Attach` directives become `PackageExports` layers. `Source` directives
/// are resolved via the callback, which returns the full set of layers the
/// sourced file contributes: its `FileExports`, any `PackageExports` from
/// `library()` calls it contains, and layers from nested `source()` calls.
/// The callback receives the raw path string from the `source()` call.
///
/// All layers produced by a single directive share that directive's offset.
pub fn directive_layers(
    directives: &[Directive],
    resolve_source: impl Fn(&str) -> Option<Vec<BindingSource>>,
) -> Vec<(TextSize, BindingSource)> {
    let mut layers = Vec::new();
    for directive in directives {
        let offset = directive.offset();
        match directive.kind() {
            DirectiveKind::Attach(pkg) => {
                layers.push((offset, BindingSource::PackageExports(pkg.clone())));
            },
            DirectiveKind::Source(path) => {
                if let Some(source_layers) = resolve_source(path) {
                    layers.extend(source_layers.into_iter().map(|l| (offset, l)));
                }
            },
        }
    }
    layers
}

/// Build the root layers for a package from its NAMESPACE.
///
/// These go at the back of every file's scope chain:
/// - `PackageImports` from `importFrom()` directives (name → package)
/// - `PackageExports` from `import()` directives
pub fn package_root_layers(namespace: &Namespace) -> Vec<BindingSource> {
    let mut layers = Vec::new();

    if !namespace.imports.is_empty() {
        let map = namespace
            .imports
            .iter()
            .map(|imp| (imp.name.clone(), imp.package.clone()))
            .collect();
        layers.push(BindingSource::PackageImports(map));
    }

    for pkg in &namespace.package_imports {
        layers.push(BindingSource::PackageExports(pkg.clone()));
    }

    layers
}
