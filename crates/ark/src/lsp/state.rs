use std::collections::HashMap;

use aether_path::FilePath;
use anyhow::anyhow;
use oak_db::File;
use oak_db::OakDatabase;
use oak_semantic::library::Library;
use salsa::Database;
use url::Url;

use crate::lsp::config::LspConfig;
use crate::lsp::db::Analysis;
use crate::lsp::inputs::source_root::SourceRoot;
use crate::lsp::open_file::OpenFile;

#[derive(Clone, Default, Debug)]
/// The world state, i.e. all the inputs necessary for analysing or refactoring
/// code. This is a pure value. There is no interior mutability in this data
/// structure. It can be cloned and safely sent to other threads.
///
/// The main loop owns and mutates this. Background readers get a
/// [`WorldSnapshot`] instead, which holds a read-only [`Analysis`] in place of
/// the writable `OakDatabase`. This prevents background threads from reaching a
/// Salsa input setter. See [`Self::diagnostics_snapshot`] and
/// [`Self::snapshot`]. This split mirrors rust-analyzer's `GlobalState`  and
/// `GlobalStateSnapshot`.
pub(crate) struct WorldState {
    /// Salsa input tree for Oak queries.
    pub(crate) db: OakDatabase,

    /// Watched documents, keyed on the normalised [`FilePath`] form.
    /// The verbatim editor URL is preserved on each [`OpenFile::wire_url`]
    /// for wire output.
    pub(crate) open_files: HashMap<FilePath, OpenFile>,

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

    /// Full read-only snapshot for a background reader that needs the whole
    /// world (completions read `open_files` and the workspace). Same shape as
    /// `self.clone()`, but the db becomes a read-only [`Analysis`].
    pub(crate) fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            db: Analysis::new(self.db.clone()),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            root: self.root.clone(),
            library: self.library.clone(),
            config: self.config.clone(),
            open_files: self.open_files.clone(),
            workspace: self.workspace.clone(),
        }
    }

    /// Trimmed read-only snapshot for the diagnostics worker, which runs off
    /// the main loop and queries oak. Drops the open-file / workspace maps the
    /// diagnostics pass doesn't read.
    pub(crate) fn diagnostics_snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            db: Analysis::new(self.db.clone()),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            root: self.root.clone(),
            library: self.library.clone(),
            config: self.config.clone(),
            open_files: HashMap::new(),
            workspace: Workspace::default(),
        }
    }
    /// Advance the oak revision without changing any oak input.
    ///
    /// Currently used for state that lives on `WorldState` but not in the Oak
    /// DB (e.g. console scopes and the diagnostics config). The revision bump
    /// invalidates in-flight background workers (e.g. diagnostics), and
    /// triggers a diagnostic refresh.
    pub(crate) fn bump_revision(&mut self) {
        self.db.synthetic_write(salsa::Durability::LOW);
    }

    pub(crate) fn open_file_mut(&mut self, uri: &Url) -> anyhow::Result<&mut OpenFile> {
        let key = FilePath::from_url(uri);
        if let Some(open_file) = self.open_files.get_mut(&key) {
            Ok(open_file)
        } else {
            Err(anyhow!("Can't find document for URI {uri}"))
        }
    }

    /// The stored [`OpenFile`] for a request.
    pub(crate) fn open_file(&self, uri: &Url) -> anyhow::Result<&OpenFile> {
        let key = FilePath::from_url(uri);
        let Some(open_file) = self.open_files.get(&key) else {
            return Err(anyhow!("Can't find document for URI {uri}"));
        };
        Ok(open_file)
    }

    /// URL to put on the wire for `file`. Open buffers keep the editor's
    /// verbatim URL so the frontend sees the URI it sent us. Files that were
    /// never opened in the editor (disk-scanned files, resolution targets) have
    /// no verbatim URL, so synthesise one from the normalised path.
    pub(crate) fn wire_url(&self, file: File) -> Url {
        let path = file.path(&self.db);
        self.open_files
            .get(path)
            .map(|open_file| open_file.wire_url().clone())
            .unwrap_or_else(|| path.to_url())
    }

    /// Register an editor buffer in `open_files`, keying on the normalised
    /// [`FilePath`] and stashing the verbatim editor URL on [`OpenFile::wire_url`] for
    /// wire output.
    ///
    /// The caller is in charge of pushing the contents into `oak` via
    /// `upsert_editor()` and handing us the resulting [`File`].
    pub(crate) fn insert_open_file(&mut self, url: Url, file: File, version: Option<i32>) {
        let key = FilePath::from_url(&url);
        let open_file = OpenFile::new(file, version, url);
        self.open_files.insert(key, open_file);
    }
}

/// Read-only snapshot of [`WorldState`] handed to background readers (e.g.
/// diagnostics). Holds a read-only [`Analysis`] in place of the writable
/// `OakDatabase`, so a reader thread can't reach Salsa input setters. Carries
/// only the fields readers actually use. Mirrors rust-analyzer's
/// `GlobalStateSnapshot`.
#[derive(Clone, Debug)]
pub(crate) struct WorldSnapshot {
    pub(crate) db: Analysis,
    pub(crate) open_files: HashMap<FilePath, OpenFile>,
    pub(crate) workspace: Workspace,
    pub(crate) console_scopes: Vec<Vec<String>>,
    pub(crate) installed_packages: Vec<String>,
    pub(crate) root: Option<SourceRoot>,
    pub(crate) library: Library,
    pub(crate) config: LspConfig,
}

impl WorldSnapshot {
    /// URL to put on the wire for `file`. Same rule as [`WorldState::wire_url`],
    /// reading through the [`Analysis`] handle.
    pub(crate) fn wire_url(&self, file: File) -> Url {
        let path = file.path(self.db.read());
        self.open_files
            .get(path)
            .map(|open_file| open_file.wire_url().clone())
            .unwrap_or_else(|| path.to_url())
    }
}

pub(crate) fn open_file_wire_urls(state: &WorldState) -> Vec<Url> {
    state
        .open_files
        .values()
        .map(|doc| doc.wire_url().clone())
        .collect()
}
