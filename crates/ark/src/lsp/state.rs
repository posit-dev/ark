use std::collections::HashMap;

use aether_path::FilePath;
use anyhow::anyhow;
use oak_db::File;
use oak_db::OakDatabase;
use url::Url;

use crate::lsp::config::LspConfig;
use crate::lsp::db::ArkDb;
use crate::lsp::open_file::OpenFile;

#[derive(Clone, Default, Debug)]
/// The world state, i.e. all the inputs necessary for analysing or refactoring
/// code. This is a pure value. There is no interior mutability in this data
/// structure. It can be cloned and safely sent to other threads.
///
/// The main loop owns and mutates this. Background readers get a
/// [`WorldStateSnapshot`] instead, which only lends its database out as
/// `&dyn ArkDb`. This prevents background threads from reaching a Salsa input
/// setter. See [`Self::diagnostics_snapshot`] and [`Self::snapshot`]. This split
/// mirrors rust-analyzer's `GlobalState` and `GlobalStateSnapshot`.
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

    pub(crate) config: LspConfig,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct Workspace {
    pub folders: Vec<Url>,
}

impl WorldState {
    pub(crate) fn new(db: OakDatabase) -> Self {
        Self {
            db,
            ..Default::default()
        }
    }

    /// Full read-only snapshot for a background reader that needs more than the
    /// db, e.g. completions read `open_files` and the workspace. Same shape as
    /// `self.clone()`, but the db is only reachable as `&dyn ArkDb`.
    pub(crate) fn snapshot(&self) -> WorldStateSnapshot {
        WorldStateSnapshot {
            db: self.db.clone(),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            config: self.config.clone(),
            open_files: self.open_files.clone(),
            workspace: self.workspace.clone(),
        }
    }

    /// Trimmed read-only snapshot for the diagnostics worker, which runs off
    /// the main loop and queries oak. Drops the open-file / workspace maps the
    /// diagnostics pass doesn't read.
    pub(crate) fn diagnostics_snapshot(&self) -> WorldStateSnapshot {
        WorldStateSnapshot {
            db: self.db.clone(),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            config: self.config.clone(),
            open_files: HashMap::new(),
            workspace: Workspace::default(),
        }
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
/// diagnostics), so a reader thread can't reach Salsa input setters. Carries only
/// the fields readers actually use. Mirrors rust-analyzer's
/// `GlobalStateSnapshot`.
#[derive(Clone, Debug)]
pub(crate) struct WorldStateSnapshot {
    /// Private so readers can only reach it through [`Self::db`].
    db: OakDatabase,
    pub(crate) open_files: HashMap<FilePath, OpenFile>,
    pub(crate) workspace: Workspace,
    pub(crate) console_scopes: Vec<Vec<String>>,
    pub(crate) installed_packages: Vec<String>,
    pub(crate) config: LspConfig,
}

impl WorldStateSnapshot {
    /// Read-only access to the database. Returns `&dyn ArkDb` rather than
    /// `&OakDatabase` because `dyn ArkDb` is unsized, so a reader can't
    /// `.clone()` its way to an owned database and call setters on it.
    pub(crate) fn db(&self) -> &dyn ArkDb {
        &self.db
    }

    /// The database's salsa cancellation token. Read-side only: it observes and
    /// arms cancellation, it doesn't mutate any input. Only cancellation tests
    /// arm it by hand.
    #[cfg(test)]
    pub(crate) fn cancellation_token(&self) -> salsa::CancellationToken {
        salsa::Database::cancellation_token(&self.db)
    }

    /// URL to put on the wire for `file`. Same rule as [`WorldState::wire_url`].
    pub(crate) fn wire_url(&self, file: File) -> Url {
        let path = file.path(self.db());
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
