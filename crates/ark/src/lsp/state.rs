use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use anyhow::anyhow;
use biome_rowan::TextSize;
use oak_core::file::list_r_files;
use oak_ide::ExternalScope;
use oak_index::external::file_layers;
use oak_index::external::package_root_layers;
use oak_index::external::ScopeLayer;
use oak_index::semantic_index::DirectiveKind;
use oak_package::library::Library;
use stdext::result::ResultExt;
use url::Url;

use crate::lsp::config::LspConfig;
use crate::lsp::document::Document;
use crate::lsp::inputs::source_root::SourceRoot;

#[derive(Clone, Default, Debug)]
/// The world state, i.e. all the inputs necessary for analysing or refactoring
/// code. This is a pure value. There is no interior mutability in this data
/// structure. It can be cloned and safely sent to other threads.
pub(crate) struct WorldState {
    /// Watched documents
    pub(crate) documents: HashMap<Url, Document>,

    /// Watched folders
    pub(crate) workspace: Workspace,

    /// Virtual documents that the LSP serves as a text document content provider for
    /// Maps a `String` uri to the contents of the document
    pub(crate) virtual_documents: HashMap<String, String>,

    /// The scopes for the console. This currently contains a list (outer `Vec`)
    /// of names (inner `Vec`) within the environments on the search path, starting
    /// from the global environment and ending with the base package. Eventually
    /// this might also be populated with the scope for the current environment
    /// in debug sessions (not implemented yet).
    ///
    /// This is currently one of the main sources of known symbols for
    /// diagnostics. In the future we should better delineate interactive
    /// contexts (e.g. the console, but scripts might also be treated as
    /// interactive, which could be a user setting) and non-interactive ones
    /// (e.g. a package). In non-interactive contexts, the lexical scopes
    /// examined for diagnostics should be fully determined by variable bindings
    /// and imports (code-first diagnostics).
    ///
    /// In the future this should probably become more complex with a list of
    /// either symbol names (as is now the case) or named environments, such as
    /// `pkg:ggplot2`. Storing named environments here will allow the LSP to
    /// retrieve the symbols in a pull fashion (the whole console scopes are
    /// currently pushed to the LSP), and cache the symbols with Salsa. The
    /// performance is not currently an issue but this could change once we do
    /// more analysis of symbols in the search path.
    pub(crate) console_scopes: Vec<Vec<String>>,

    /// Currently installed packages
    pub(crate) installed_packages: Vec<String>,

    /// The root of the source tree (e.g., a package).
    pub(crate) root: Option<SourceRoot>,

    /// Map of package name to package metadata for installed libraries. Lazily populated.
    pub(crate) library: Library,

    pub(crate) config: LspConfig,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct Workspace {
    pub folders: Vec<Url>,
}

impl WorldState {
    pub(crate) fn get_document(&self, uri: &Url) -> anyhow::Result<&Document> {
        if let Some(doc) = self.documents.get(uri) {
            Ok(doc)
        } else {
            Err(anyhow!("Can't find document for URI {uri}"))
        }
    }

    pub(crate) fn get_document_mut(&mut self, uri: &Url) -> anyhow::Result<&mut Document> {
        if let Some(doc) = self.documents.get_mut(uri) {
            Ok(doc)
        } else {
            Err(anyhow!("Can't find document for URI {uri}"))
        }
    }

    /// Create a scope chain for a particular file, taking into account the
    /// current project type. For packages, this creates a scope containing
    /// imports and top-level definitions in other files, respecting the
    /// collation order.
    pub(crate) fn file_scope(&self, file: &Url) -> ExternalScope {
        let Some(SourceRoot::Package(ref pkg)) = self.root else {
            let directives = self.directive_layers(file);
            return ExternalScope::search_path(directives, default_search_path());
        };

        let root_layers = package_root_layers(&pkg.namespace);
        let r_dir = pkg.path.join("R");

        // If there is a collation field, we use it as an authoritative source
        // for the files in the package (and the order in which they are loaded)
        let ordered: Vec<PathBuf> = pkg
            .description
            .collate()
            .map(|names| names.into_iter().map(|n| r_dir.join(n)).collect())
            .unwrap_or_else(|| {
                // No collation field, list R files and sort in C order
                // (R's default collation)
                let mut paths = list_r_files(&r_dir);
                paths.sort();
                paths
            });

        let Some(file_path) = file.to_file_path().ok() else {
            return ExternalScope::default();
        };

        // Iterate in reverse collation order so later files (which shadow
        // earlier ones) come first in the chain. Split at the current file
        // to separate predecessors from the full set.
        let mut top_level = Vec::new();
        let mut lazy = Vec::new();
        let mut past_current = false;

        for path in ordered.iter().rev() {
            if *path == file_path {
                past_current = true;
                continue;
            }

            let Some(uri) = Url::from_file_path(path).log_err() else {
                continue;
            };

            // Use the open document if available, otherwise read from disk.
            // TODO: Store non-opened workspace documents in VFS.
            let doc = if let Some(open) = self.documents.get(&uri) {
                open
            } else {
                let Some(contents) = std::fs::read_to_string(path).log_err() else {
                    continue;
                };
                &Document::new(&contents, None)
            };

            let layers = file_layers(uri, &doc.semantic_index());
            lazy.extend(layers.clone());
            if past_current {
                top_level.extend(layers);
            }
        }

        // Add the base namespace, which is the only base package in scope by default
        top_level.extend(root_layers.clone());
        top_level.push(ScopeLayer::PackageExports("base".to_string()));
        lazy.extend(root_layers);
        lazy.push(ScopeLayer::PackageExports("base".to_string()));

        ExternalScope::package(top_level, lazy)
    }

    fn directive_layers(&self, file: &Url) -> Vec<(TextSize, ScopeLayer)> {
        let Some(doc) = self.documents.get(file) else {
            return Vec::new();
        };
        let index = doc.semantic_index();
        index
            .file_directives()
            .iter()
            .map(|d| match d.kind() {
                DirectiveKind::Attach(pkg) => (d.offset(), ScopeLayer::PackageExports(pkg.clone())),
            })
            .collect()
    }
}

/// The default R search path for scripts: the default packages that R
/// attaches on startup, in search order (last attached = searched first).
fn default_search_path() -> Vec<ScopeLayer> {
    // R's default packages, in reverse attachment order (most recently
    // attached first). These are always on the search path unless
    // overridden by `R_DEFAULT_PACKAGES`.
    let default_packages = [
        "utils",
        "stats",
        "datasets",
        "methods",
        "grDevices",
        "graphics",
        "base",
    ];
    default_packages
        .into_iter()
        .map(|pkg| ScopeLayer::PackageExports(pkg.to_string()))
        .collect()
}

pub(crate) fn with_document<T, F>(
    path: &Path,
    state: &WorldState,
    mut callback: F,
) -> anyhow::Result<T>
where
    F: FnMut(&Document) -> anyhow::Result<T>,
{
    let mut fallback = || {
        let contents = std::fs::read_to_string(path)?;
        let document = Document::new(contents.as_str(), None);
        callback(&document)
    };

    // If we have a cached copy of the document (because we're monitoring it)
    // then use that; otherwise, try to read the document from the provided
    // path and use that instead.
    let Ok(uri) = Url::from_file_path(path) else {
        log::info!(
            "couldn't construct uri from {}; reading from disk instead",
            path.display()
        );
        return fallback();
    };

    let Ok(document) = state.get_document(&uri) else {
        log::info!("no document for uri {uri}; reading from disk instead");
        return fallback();
    };

    callback(document)
}

pub(crate) fn workspace_uris(state: &WorldState) -> Vec<Url> {
    let uris: Vec<Url> = state.documents.iter().map(|elt| elt.0.clone()).collect();
    uris
}

#[cfg(test)]
mod tests {
    use biome_rowan::TextSize;
    use oak_index::external::ScopeLayer;
    use stdext::assert_not;

    use super::*;
    use crate::lsp::document::Document;
    use crate::lsp::util::test_path;

    fn make_state(uri: &Url, doc: &Document) -> WorldState {
        let mut state = WorldState::default();
        state.documents.insert(uri.clone(), doc.clone());
        state
    }

    fn has_package(layers: &[ScopeLayer], name: &str) -> bool {
        layers
            .iter()
            .any(|l| matches!(l, ScopeLayer::PackageExports(p) if p == name))
    }

    #[test]
    fn test_script_library_directive_position_sensitive() {
        // At top-level, `library()` is position-sensitive: code before
        // the call should not see the package, code after should.
        // The lazy scope (used for completions etc.) sees all directives.
        let code = "inform('hi')\nlibrary(rlang)\ninform('hello')\n";
        let doc = Document::new(code, None);
        let uri = test_path("script.R");
        let state = make_state(&uri, &doc);

        let scope = state.file_scope(&uri);
        let index = doc.semantic_index();

        let before = scope.at(&index, TextSize::from(0));
        assert_not!(has_package(&before, "rlang"));

        let after = scope.at(&index, TextSize::from(code.rfind("inform").unwrap() as u32));
        assert!(has_package(&after, "rlang"));

        assert!(has_package(scope.lazy(), "rlang"));
    }

    #[test]
    fn test_script_library_directive_visible_in_function_before_call() {
        // Function bodies see all directives regardless of position,
        // because the function will typically be called after the
        // script has been fully sourced.
        let code = "f <- function() inform('hello')\nlibrary(rlang)\n";
        let doc = Document::new(code, None);
        let uri = test_path("script.R");
        let state = make_state(&uri, &doc);

        let scope = state.file_scope(&uri);
        let index = doc.semantic_index();

        let in_function = scope.at(&index, TextSize::from(code.find("inform").unwrap() as u32));
        assert!(has_package(&in_function, "rlang"));
    }

    #[test]
    fn test_script_without_library_no_extra_packages() {
        let code = "inform('hello')\n";
        let doc = Document::new(code, None);
        let uri = test_path("script.R");
        let state = make_state(&uri, &doc);

        let scope = state.file_scope(&uri);
        let layers = scope.lazy();
        assert_not!(has_package(layers, "rlang"));
    }
}
