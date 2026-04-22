use biome_rowan::TextSize;
use oak_index::external::BindingSource;
use oak_index::semantic_index::ScopeKind;
use oak_index::semantic_index::SemanticIndex;

/// The external scope chain for a file, determined by its project context.
#[derive(Debug)]
pub enum FileScope {
    /// File inside an R package. The scope chain includes layers from other
    /// package files (ordered by collation, later files shadow earlier ones)
    /// and from NAMESPACE imports (`importFrom`, `import`), with base
    /// at the bottom. Top-level code only sees predecessor files
    /// whereas function bodies (lazy scopes) see all files because the
    /// namespace is fully populated before any function runs.
    Package {
        top_level: Vec<BindingSource>,
        lazy: Vec<BindingSource>,
    },

    /// Script or file outside a package. The scope chain is the R
    /// search path: `library()` attachments from the file itself,
    /// default packages (stats, graphics, etc.), and base.
    SearchPath(Vec<BindingSource>),
}

impl Default for FileScope {
    fn default() -> Self {
        Self::SearchPath(Vec::new())
    }
}

impl FileScope {
    pub fn package(top_level: Vec<BindingSource>, lazy: Vec<BindingSource>) -> Self {
        Self::Package { top_level, lazy }
    }

    pub fn search_path(layers: Vec<BindingSource>) -> Self {
        Self::SearchPath(layers)
    }

    /// Return the scope chain appropriate for the given offset. For
    /// packages, top-level scope uses predecessors only while lazy
    /// (function) scopes see all files. For scripts, the same search
    /// path applies everywhere.
    pub fn at(&self, index: &SemanticIndex, offset: TextSize) -> &[BindingSource] {
        match self {
            Self::Package { top_level, lazy } => {
                let (_, scope) = index.scope_at(offset);
                match scope.kind() {
                    ScopeKind::File => top_level,
                    ScopeKind::Function => lazy,
                }
            },
            Self::SearchPath(layers) => layers,
        }
    }

    /// The full scope for lazy contexts. Useful for features that don't
    /// have a cursor position (e.g. workspace symbols).
    pub fn lazy(&self) -> &[BindingSource] {
        match self {
            Self::Package { lazy, .. } => lazy,
            Self::SearchPath(layers) => layers,
        }
    }
}
