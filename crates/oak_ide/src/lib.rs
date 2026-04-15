mod goto_definition;

use biome_rowan::TextRange;
use biome_rowan::TextSize;
pub use goto_definition::goto_definition;
use oak_index::external::BindingSource;
use oak_index::semantic_index::ScopeKind;
use oak_index::semantic_index::SemanticIndex;
use url::Url;

/// The external scope chain for a file, determined by its project context.
#[derive(Debug, Default)]
pub enum FileScope {
    /// File inside an R package. The scope chain includes layers from
    /// other package files (ordered by collation, later files shadow
    /// earlier ones) and from NAMESPACE imports (`importFrom`,
    /// `import`). Top-level code only sees predecessor files, function
    /// bodies (lazy scopes) see all files because the namespace is
    /// fully populated before any function runs.
    Package {
        top_level: Vec<BindingSource>,
        lazy: Vec<BindingSource>,
    },

    /// Script or file outside a package. No cross-file scope.
    #[default]
    Isolated,
}

impl FileScope {
    pub fn package(top_level: Vec<BindingSource>, lazy: Vec<BindingSource>) -> Self {
        Self::Package { top_level, lazy }
    }

    /// Return the scope chain appropriate for the given offset: top-level
    /// scope uses predecessors only, lazy (function) scopes see all files.
    pub fn at(&self, index: &SemanticIndex, offset: TextSize) -> &[BindingSource] {
        match self {
            Self::Package { top_level, lazy } => {
                let scope = index.scope_at(offset);
                match index.scope(scope).kind() {
                    ScopeKind::File => top_level,
                    ScopeKind::Function => lazy,
                }
            },
            Self::Isolated => &[],
        }
    }

    /// The full scope for lazy contexts. Useful for features that don't have a
    /// cursor position (e.g. workspace symbols) and need a conservative scope.
    pub fn lazy(&self) -> &[BindingSource] {
        match self {
            Self::Package { lazy, .. } => lazy,
            Self::Isolated => &[],
        }
    }
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
