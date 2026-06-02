use std::collections::HashMap;

use aether_path::FilePath;
use anyhow::anyhow;
use oak_db::File;
use oak_db::OakDatabase;
use oak_semantic::library::Library;
use url::Url;

use crate::lsp::ark_file::ArkFile;
use crate::lsp::config::DocumentConfig;
use crate::lsp::config::LspConfig;
use crate::lsp::inputs::source_root::SourceRoot;

#[derive(Clone, Default, Debug)]
/// The world state, i.e. all the inputs necessary for analysing or refactoring
/// code. This is a pure value. There is no interior mutability in this data
/// structure. It can be cloned and safely sent to other threads.
pub(crate) struct WorldState {
    /// Salsa input tree for Oak queries.
    pub(crate) db: OakDatabase,

    /// Watched documents, keyed on the normalised [`FilePath`] form.
    /// The verbatim editor URL is preserved on each [`ArkFile::url`]
    /// for wire output.
    pub(crate) open_files: HashMap<FilePath, ArkFile>,

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

    pub(crate) fn ark_file_mut(&mut self, uri: &Url) -> anyhow::Result<&mut ArkFile> {
        let key = FilePath::from_url(uri);
        if let Some(ark_file) = self.open_files.get_mut(&key) {
            Ok(ark_file)
        } else {
            Err(anyhow!("Can't find document for URI {uri}"))
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
            open_files: HashMap::new(),
            virtual_documents: HashMap::new(),
            workspace: Workspace::default(),
        }
    }

    /// Get a clone of the stored [`ArkFile`] for a request.
    ///
    /// `ArkFile` is cheap to clone: the analysis handle is a salsa id and the
    /// protocol fields are small. Handlers want an owned value because the
    /// `r_task` ones move it across a thread boundary.
    pub(crate) fn ark_file(&self, uri: &Url) -> anyhow::Result<ArkFile> {
        let key = FilePath::from_url(uri);
        let Some(ark_file) = self.open_files.get(&key) else {
            return Err(anyhow!("Can't find document for URI {uri}"));
        };
        Ok(ark_file.clone())
    }

    /// Register an editor buffer in `open_files`, keying on the normalised
    /// [`FilePath`] and stashing the verbatim editor URL on [`ArkFile::url`] for
    /// wire output.
    ///
    /// The caller is in charge of pushing the contents into `oak` via
    /// `upsert_editor()` and handing us the resulting [`File`].
    pub(crate) fn insert_ark_file(&mut self, uri: Url, file: File, version: Option<i32>) {
        let key = FilePath::from_url(&uri);
        let ark_file = ArkFile {
            file,
            version,
            config: DocumentConfig::default(),
            url: uri,
            encoding: self.config.position_encoding,
        };
        self.open_files.insert(key, ark_file);
    }
}

pub(crate) fn workspace_uris(state: &WorldState) -> Vec<Url> {
    state
        .open_files
        .values()
        .map(|doc| doc.url.clone())
        .collect()
}
