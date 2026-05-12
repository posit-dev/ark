use std::borrow::Cow;

use biome_rowan::TextSize;
use oak_index::scope_layer::ScopeLayer;
use oak_index::semantic_index::Directive;
use oak_index::semantic_index::DirectiveKind;
use oak_index::semantic_index::ScopeKind;
use oak_index::semantic_index::SemanticIndex;
use oak_index::ScopeId;

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
    /// At top-level, only directives that appear before the cursor are
    /// visible (R executes scripts sequentially). Inside function bodies
    /// all directives are visible because the function will typically be
    /// called after the full script has been sourced.
    SearchPath {
        base: Vec<ScopeLayer>,
        directives: Vec<Directive>,
    },
}

impl Default for ExternalScope {
    fn default() -> Self {
        Self::SearchPath {
            base: Vec::new(),
            directives: Vec::new(),
        }
    }
}

impl ExternalScope {
    pub fn package(top_level: Vec<ScopeLayer>, lazy: Vec<ScopeLayer>) -> Self {
        Self::Package { top_level, lazy }
    }

    pub fn search_path(directives: Vec<Directive>, base: Vec<ScopeLayer>) -> Self {
        Self::SearchPath { base, directives }
    }

    /// Return the scope chain appropriate for the given offset. For
    /// packages, top-level scope uses predecessors only while lazy
    /// (function) scopes see all files. For scripts, top-level code
    /// only sees `library()` calls that precede the cursor while
    /// function bodies see all directives.
    pub fn at(&self, index: &SemanticIndex, offset: TextSize) -> Cow<'_, [ScopeLayer]> {
        match self {
            Self::Package { top_level, lazy } => {
                let (_, scope) = index.scope_at(offset);
                match scope.kind() {
                    ScopeKind::File => Cow::Borrowed(top_level),
                    ScopeKind::Function => Cow::Borrowed(lazy),
                }
            },
            Self::SearchPath { base, directives } => {
                let (cursor_scope, _) = index.scope_at(offset);
                let file_scope = ScopeId::from(0);
                let in_function = cursor_scope != file_scope;
                let layers: Vec<_> = directives
                    .iter()
                    .rev()
                    .filter(|d| {
                        let dir_scope = d.scope();
                        // File-scope directives are always visible inside
                        // function bodies (the function is typically called
                        // after the full script has been sourced).
                        if in_function && dir_scope == file_scope {
                            return true;
                        }
                        d.offset() < offset &&
                            index.ancestor_scopes(cursor_scope).any(|s| s == dir_scope)
                    })
                    .map(|d| match d.kind() {
                        DirectiveKind::Attach(pkg) => ScopeLayer::PackageExports(pkg.clone()),
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
                directives, base, ..
            } => {
                let file_scope = ScopeId::from(0);
                let mut layers: Vec<ScopeLayer> = directives
                    .iter()
                    .rev()
                    .filter(|d| d.scope() == file_scope)
                    .map(|d| match d.kind() {
                        DirectiveKind::Attach(pkg) => ScopeLayer::PackageExports(pkg.clone()),
                    })
                    .collect();
                layers.extend(base.iter().cloned());
                Cow::Owned(layers)
            },
        }
    }
}
