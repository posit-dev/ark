use std::borrow::Cow;

use biome_rowan::TextSize;
use oak_semantic::scope_layer::ScopeLayer;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticCall;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::ScopeId;

/// The external scope chain for a file, determined by its project context.
#[derive(Debug)]
pub enum ExternalScope {
    /// File inside an R package. The scope chain includes layers from other
    /// package files (ordered by collation, later files shadow earlier ones)
    /// and from NAMESPACE imports (`importFrom`, `import`), with base
    /// at the bottom. Top-level code only sees predecessor files
    /// whereas function bodies (lazy scopes) see all files because the
    /// namespace is fully populated before any function runs.
    Package {
        top_level: Vec<ScopeLayer>,
        lazy: Vec<ScopeLayer>,
    },

    /// Script or file outside a package. The scope chain is the R
    /// search path: `library()` attachments from the file itself,
    /// default packages (stats, graphics, etc.), and base.
    ///
    /// At top-level, only `Attach` semantic calls that appear before
    /// the cursor are visible (R executes scripts sequentially).
    /// Inside function bodies all visible attachments are exposed
    /// because the function will typically be called after the full
    /// script has been sourced.
    SearchPath {
        base: Vec<ScopeLayer>,
        semantic_calls: Vec<SemanticCall>,
    },
}

impl Default for ExternalScope {
    fn default() -> Self {
        Self::SearchPath {
            base: Vec::new(),
            semantic_calls: Vec::new(),
        }
    }
}

impl ExternalScope {
    pub fn package(top_level: Vec<ScopeLayer>, lazy: Vec<ScopeLayer>) -> Self {
        Self::Package { top_level, lazy }
    }

    pub fn search_path(semantic_calls: Vec<SemanticCall>, base: Vec<ScopeLayer>) -> Self {
        Self::SearchPath {
            base,
            semantic_calls,
        }
    }

    /// Return the scope chain appropriate for the given offset. For
    /// packages, top-level scope uses predecessors only while lazy
    /// (function) scopes see all files. For scripts, top-level code
    /// only sees `library()` calls that precede the cursor while
    /// function bodies see all attachments.
    pub fn at(&self, index: &SemanticIndex, offset: TextSize) -> Cow<'_, [ScopeLayer]> {
        match self {
            Self::Package { top_level, lazy } => {
                let (_, scope) = index.scope_at(offset);
                match scope.kind() {
                    ScopeKind::File => Cow::Borrowed(top_level),
                    ScopeKind::Function => Cow::Borrowed(lazy),
                }
            },
            Self::SearchPath {
                base,
                semantic_calls,
            } => {
                let (cursor_scope, _) = index.scope_at(offset);
                let file_scope = ScopeId::from(0);
                let in_function = cursor_scope != file_scope;
                let layers: Vec<_> = semantic_calls
                    .iter()
                    .rev()
                    .filter(|c| {
                        let call_scope = c.scope();
                        // File-scope attachments are always visible inside
                        // function bodies (the function is typically called
                        // after the full script has been sourced).
                        if in_function && call_scope == file_scope {
                            return true;
                        }
                        c.offset() < offset &&
                            index.ancestor_scopes(cursor_scope).any(|s| s == call_scope)
                    })
                    .filter_map(|c| match c.kind() {
                        SemanticCallKind::Attach { package } => {
                            Some(ScopeLayer::PackageExports(package.clone()))
                        },
                        SemanticCallKind::Source { .. } => None,
                    })
                    .chain(base.iter().cloned())
                    .collect();
                Cow::Owned(layers)
            },
        }
    }

    /// The full scope for lazy contexts. Useful for features that don't
    /// have a cursor position (e.g. completions, workspace symbols).
    pub fn lazy(&self) -> Cow<'_, [ScopeLayer]> {
        match self {
            Self::Package { lazy, .. } => Cow::Borrowed(lazy),
            Self::SearchPath {
                semantic_calls,
                base,
                ..
            } => {
                let file_scope = ScopeId::from(0);
                let mut layers: Vec<ScopeLayer> = semantic_calls
                    .iter()
                    .rev()
                    .filter(|c| c.scope() == file_scope)
                    .filter_map(|c| match c.kind() {
                        SemanticCallKind::Attach { package } => {
                            Some(ScopeLayer::PackageExports(package.clone()))
                        },
                        SemanticCallKind::Source { .. } => None,
                    })
                    .collect();
                layers.extend(base.iter().cloned());
                Cow::Owned(layers)
            },
        }
    }
}
