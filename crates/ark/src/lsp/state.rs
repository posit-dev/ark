use std::collections::HashMap;

use aether_path::FilePath;
use anyhow::anyhow;
use oak_db::Db;
use oak_db::OakDatabase;
use oak_semantic::library::Library;
use url::Url;

use crate::lsp::ark_file::ArkFile;
use crate::lsp::config::LspConfig;
use crate::lsp::document::Document;
use crate::lsp::inputs::source_root::SourceRoot;

#[derive(Clone, Default, Debug)]
/// The world state, i.e. all the inputs necessary for analysing or refactoring
/// code. This is a pure value. There is no interior mutability in this data
/// structure. It can be cloned and safely sent to other threads.
pub(crate) struct WorldState {
    /// Salsa input tree for Oak queries.
    pub(crate) db: OakDatabase,

    /// Watched documents, keyed on the normalised [`FilePath`] form.
    /// The verbatim editor URL is preserved on each [`Document::url`]
    /// for wire output.
    pub(crate) documents: HashMap<FilePath, Document>,

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

    /// Map of package name to package metadata and package sources for installed
    /// libraries. Lazily populated.
    pub(crate) library: Library,

    pub(crate) config: LspConfig,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct Workspace {
    pub folders: Vec<Url>,
}

impl WorldState {
    pub(crate) fn new(db: OakDatabase, library: Library) -> Self {
        Self {
            db,
            library,
            ..Default::default()
        }
    }

    pub(crate) fn get_document(&self, path: &FilePath) -> anyhow::Result<&Document> {
        if let Some(doc) = self.documents.get(path) {
            Ok(doc)
        } else {
            Err(anyhow!("Can't find document for path {path}"))
        }
    }

    pub(crate) fn get_document_mut(&mut self, path: &FilePath) -> anyhow::Result<&mut Document> {
        if let Some(doc) = self.documents.get_mut(path) {
            Ok(doc)
        } else {
            Err(anyhow!("Can't find document for path {path}"))
        }
    }

    /// Snapshot for the diagnostics worker, which runs off the main loop and
    /// queries oak.
    pub(crate) fn diagnostics_snapshot(&self) -> WorldState {
        WorldState {
            db: self.db.clone(),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            root: self.root.clone(),
            library: self.library.clone(),
            config: self.config.clone(),
            documents: HashMap::new(),
            virtual_documents: HashMap::new(),
            workspace: Workspace::default(),
        }
    }

    /// Build an [`ArkFile`] for a request.
    ///
    /// Most fields come from the legacy `Document` struct, namely `version`,
    /// `config`, and `url`. The `encoding` comes from the world config instead.
    /// The analysis handle comes from the matching `oak_db::File`.
    ///
    /// The `Document` and the `File` are kept in sync by the editor bridge,
    /// which calls `upsert_editor()` on every `did_open` and `did_change`. So a
    /// `File` exists whenever a `Document` does.
    pub(crate) fn ark_file(&self, uri: &Url) -> anyhow::Result<ArkFile> {
        let key = FilePath::from_url(uri);
        let document = self.get_document(&key)?;
        let Some(file) = self.db.file_by_path(&key) else {
            return Err(anyhow!("No `oak_db` file for URI {uri}"));
        };
        Ok(ArkFile {
            file,
            version: document.version,
            config: document.config.clone(),
            url: document.url.clone(),
            encoding: self.config.position_encoding,
        })
    }

    /// Insert a document, keying on the normalised [`FilePath`] and
    /// stashing the verbatim editor URL on [`Document::url`] for wire
    /// output.
    pub(crate) fn insert_document(&mut self, uri: Url, mut doc: Document) {
        let key = FilePath::from_url(&uri);
        doc.url = uri;
        self.documents.insert(key, doc);
    }
}

pub(crate) fn workspace_uris(state: &WorldState) -> Vec<Url> {
    state
        .documents
        .values()
        .map(|doc| doc.url.clone())
        .collect()
}
