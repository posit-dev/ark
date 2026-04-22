mod file_scope;
mod goto_definition;

use biome_rowan::TextRange;
pub use file_scope::ExternalScope;
pub use goto_definition::goto_definition;
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
