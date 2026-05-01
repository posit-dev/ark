use biome_rowan::TextRange;
use oak_package::definitions::PackageDefinitionVisibility;
use oak_package::library::Library;
use stdext::result::ResultExt;
use url::Url;

use crate::scope_layer::ScopeLayer;

/// The result of resolving a name against the external scope chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalDefinition {
    file: Url,
    name: String,
    range: TextRange,
}

impl ExternalDefinition {
    pub fn into_parts(self) -> (Url, String, TextRange) {
        (self.file, self.name, self.range)
    }

    pub fn file(&self) -> &Url {
        &self.file
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn range(&self) -> TextRange {
        self.range
    }
}

/// Walk the scope chain front-to-back, returning the first match.
pub fn resolve_external_name(
    library: &Library,
    scope: &[ScopeLayer],
    name: &str,
) -> Option<ExternalDefinition> {
    for source in scope {
        match source {
            ScopeLayer::FileExports { file, exports } => {
                if let Some(range) = exports.get(name) {
                    return Some(ExternalDefinition {
                        file: file.clone(),
                        name: name.to_string(),
                        range: *range,
                    });
                }
            },

            ScopeLayer::PackageImports(names) => {
                if let Some(pkg) = names.get(name) {
                    if let Some(def) = resolve_in_package(
                        name,
                        pkg,
                        PackageDefinitionVisibility::Exported,
                        library,
                    ) {
                        return Some(def);
                    }
                }
            },

            ScopeLayer::PackageExports(pkg) => {
                if let Some(def) =
                    resolve_in_package(name, pkg, PackageDefinitionVisibility::Exported, library)
                {
                    return Some(def);
                }
            },
        }
    }

    None
}

/// Resolve a name in a specific package's exported symbols.
pub fn resolve_in_package(
    name: &str,
    package: &str,
    visibility: PackageDefinitionVisibility,
    library: &Library,
) -> Option<ExternalDefinition> {
    // FIXME: Slow without salsa!
    let package_definitions = library.definitions(package)?;
    let definition = package_definitions.get(name)?;

    if visibility != definition.visibility() {
        return None;
    }

    let file = package_definitions.file(definition.file_id());
    let file = Url::from_file_path(file).log_err()?;

    Some(ExternalDefinition {
        file,
        name: name.to_string(),
        range: definition.range(),
    })
}
