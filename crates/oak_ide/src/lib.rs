mod goto_definition;
mod identifier;

use biome_rowan::TextRange;
use biome_rowan::TextSize;
pub use goto_definition::goto_definition;
pub use identifier::Identifier;
use url::Url;

/// A cursor location in the workspace: a file and an offset into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOffset {
    pub file: Url,
    pub offset: TextSize,
}

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
