mod find_references;
mod goto_definition;
mod rename;

use biome_rowan::TextRange;
pub use find_references::find_references;
pub use goto_definition::goto_definition;
use oak_db::File;
pub use rename::prepare_rename;
pub use rename::rename;

/// A span in the workspace: a file and a byte range within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRange {
    pub file: File,
    pub range: TextRange,
}

/// A location in source code that the editor can navigate to.
///
/// Shared result type for IDE features like goto-definition, hover,
/// etc., where the editor distinguishes the full extent of the binding
/// from the focus (the name selection). The LSP layer converts this into
/// `LocationLink`. For features without that distinction (find-refs,
/// document-highlight), use `FileRange` directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    pub file: File,
    pub name: String,
    pub full_range: TextRange,
    pub focus_range: TextRange,
}
