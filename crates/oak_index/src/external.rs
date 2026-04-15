use std::collections::HashMap;

use biome_rowan::TextRange;
use oak_package::library::Library;
use oak_package::package_namespace::Namespace;
use url::Url;

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
                if pkg
                    .exported_symbols
                    .binary_search(&name.to_string())
                    .is_ok()
                {
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
pub fn file_layers(file: Url, index: &SemanticIndex) -> Vec<BindingSource> {
    let mut layers = Vec::new();

    // Last definition of each name wins
    let mut exports = HashMap::new();
    for (name, range) in index.file_exports() {
        exports.insert(name.to_string(), range);
    }

    layers.push(BindingSource::FileExports { file, exports });

    for directive in index.file_directives() {
        match directive.kind() {
            DirectiveKind::Attach(pkg) => {
                layers.push(BindingSource::PackageExports(pkg.clone()));
            },
        }
    }

    layers
}

/// Build the root layers for a package from its NAMESPACE.
///
/// These go at the bottom of every file's scope chain:
/// - `PackageImports` from `importFrom()` directives (name → package)
/// - `PackageExports` from `import()` directives
pub fn package_root_layers(namespace: &Namespace) -> Vec<BindingSource> {
    let mut layers = Vec::new();

    if !namespace.imports.is_empty() {
        let map = namespace
            .imports
            .iter()
            .map(|(name, pkg)| (name.clone(), pkg.clone()))
            .collect();
        layers.push(BindingSource::PackageImports(map));
    }

    for pkg in &namespace.package_imports {
        layers.push(BindingSource::PackageExports(pkg.clone()));
    }

    layers
}

/// Resolves a bare unbound function name to its external definition.
/// Used during the semantic index build to determine whether a call target
/// is an NSE function from a known package.
pub trait ExternalResolver {
    fn resolve(&self, name: &str) -> Option<ExternalDefinition>;
}

/// Resolves names against a scope chain and library. Wraps
/// `resolve_external_name` behind the `ExternalResolver` trait.
pub struct ScopeResolver<'a> {
    scope: &'a [BindingSource],
    library: &'a Library,
}

impl<'a> ScopeResolver<'a> {
    pub fn new(scope: &'a [BindingSource], library: &'a Library) -> Self {
        Self { scope, library }
    }
}

impl ExternalResolver for ScopeResolver<'_> {
    fn resolve(&self, name: &str) -> Option<ExternalDefinition> {
        resolve_external_name(self.library, self.scope, name)
    }
}

/// Noop resolver that never finds external definitions.
/// Used by `semantic_index()` when no cross-file context is available.
pub(crate) struct NoopResolver;

impl ExternalResolver for NoopResolver {
    fn resolve(&self, _name: &str) -> Option<ExternalDefinition> {
        None
    }
}
