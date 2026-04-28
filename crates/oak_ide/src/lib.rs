mod external_scope;
mod goto_definition;
mod identifier;

use biome_rowan::TextRange;
pub use external_scope::ExternalScope;
pub use goto_definition::goto_definition;
pub use identifier::Identifier;
use oak_layers::external::ExternalDefinition;
use url::Url;

/// A location in source code that the editor can navigate to.
///
/// Shared result type for IDE features like goto-definition, find-references,
/// etc. The LSP layer converts these uniformly into `LocationLink`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    pub file: Url,
    pub name: String,
    pub full_range: TextRange,
    pub focus_range: TextRange,
}

impl From<ExternalDefinition> for NavigationTarget {
    fn from(value: ExternalDefinition) -> Self {
        let (file, name, range) = value.into_parts();
        Self {
            file,
            name,
            full_range: range,
            focus_range: range,
        }
    }
}
