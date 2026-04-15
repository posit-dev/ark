use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use anyhow::anyhow;
use oak_core::file::list_r_files;
use oak_ide::FileScope;
use oak_index::external::directive_layers;
use oak_index::external::file_layers;
use oak_index::external::package_root_layers;
use oak_index::external::BindingSource;
use oak_package::collation::collation_order;
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
    pub(crate) fn file_scope(&self, file: &Url) -> FileScope {
        let Some(SourceRoot::Package(ref pkg)) = self.root else {
            return self.script_file_scope(file);
        };

        let root_layers = package_root_layers(&pkg.namespace);

        // Collect R source filenames from open documents and disk. Open
        // documents take precedence for content (handled below when building
        // layers), but we also need to discover files that only exist on disk
        // (not yet opened).
        let r_dir = pkg.path.join("R");
        let r_dir_url = Url::from_directory_path(&r_dir).ok();

        let mut filenames = HashSet::new();

        // Discover open documents
        if let Some(ref dir_url) = r_dir_url {
            for uri in self.documents.keys() {
                if uri.as_str().starts_with(dir_url.as_str()) {
                    if let Some(name) = uri.path().rsplit('/').next() {
                        filenames.insert(name.to_string());
                    }
                }
            }
        }

        // Then files on disk that aren't already known from open documents.
        for path in list_r_files(r_dir.as_ref()) {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                filenames.insert(name.to_string());
            }
        }

        let filenames: Vec<String> = filenames.into_iter().collect();
        let ordered = collation_order(&pkg.description, &filenames);

        let filename = file.path().rsplit('/').next().unwrap_or_default();

        // Iterate in reverse collation order so later files (which shadow
        // earlier ones) come first in the chain. Split at the current file
        // to separate predecessors from the full set.
        let mut top_level = Vec::new();
        let mut lazy = Vec::new();
        let mut past_current = false;

        for name in ordered.iter().rev() {
            if name.as_str() == filename {
                past_current = true;
                continue;
            }

            let path = r_dir.join(name);
            let Some(uri) = Url::from_file_path(&path).log_err() else {
                continue;
            };

            // Use the open document if available, otherwise read from disk.
            // TODO: Store non-opened workspace documents in VFS.
            let doc = if let Some(open) = self.documents.get(&uri) {
                open
            } else {
                let Ok(contents) = std::fs::read_to_string(&path) else {
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

        top_level.extend(root_layers.clone());
        top_level.push(BindingSource::PackageExports("base".to_string()));
        lazy.extend(root_layers);
        lazy.push(BindingSource::PackageExports("base".to_string()));

        FileScope::package(top_level, lazy)
    }

    /// Build the scope for a script file (not inside a package).
    ///
    /// Resolves `library()` and `source()` directives from the file's own
    /// content, then appends the default R search path.
    fn script_file_scope(&self, file: &Url) -> FileScope {
        let file_path = file.to_file_path().ok();

        let doc = if let Some(open) = self.documents.get(file) {
            open
        } else if let Some(contents) = file_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).log_err())
        {
            &Document::new(&contents, None)
        } else {
            return FileScope::search_path(Vec::new(), default_search_path());
        };

        let index = doc.semantic_index();

        let file_dir = file_path.and_then(|p| p.parent().map(|d| d.to_path_buf()));

        let directives = directive_layers(index.file_directives(), |path| {
            let dir = file_dir.as_ref()?;
            self.resolve_source_layers(dir, path)
        });

        FileScope::search_path(directives, default_search_path())
    }

    /// Resolve a `source()` directive into the full set of layers the sourced
    /// file contributes: its own exports, `PackageExports` from any
    /// `library()` calls, and layers from nested `source()` calls.
    fn resolve_source_layers(&self, base_dir: &Path, path: &str) -> Option<Vec<BindingSource>> {
        let resolved = base_dir.join(path);
        let url = Url::from_file_path(&resolved).log_err()?;

        let sourced_doc = if let Some(open) = self.documents.get(&url) {
            open
        } else {
            let contents = std::fs::read_to_string(&resolved).log_err()?;
            &Document::new(&contents, None)
        };

        let index = sourced_doc.semantic_index();

        let mut layers = Vec::new();

        let exports = index
            .file_exports()
            .into_iter()
            .map(|(name, range)| (name.to_string(), range))
            .collect();
        layers.push(BindingSource::FileExports { file: url, exports });

        // Recurse into the sourced document in case it itself calls `source()`
        let source_dir = resolved.parent()?;
        let nested = directive_layers(index.file_directives(), |nested_path| {
            self.resolve_source_layers(source_dir, nested_path)
        });
        layers.extend(nested.into_iter().map(|(_, l)| l));

        Some(layers)
    }
}

/// The default R search path for scripts: the default packages that R
/// attaches on startup, in search order (last attached = searched first).
fn default_search_path() -> Vec<BindingSource> {
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
        .map(|pkg| BindingSource::PackageExports(pkg.to_string()))
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
