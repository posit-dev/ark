use std::collections::HashMap;

use aether_path::AbsPathBuf;
use aether_path::FilePath;
use anyhow::anyhow;
use oak_db::File;
use oak_db::OakDatabase;
use salsa::Database;
use tower_lsp_server::ls_types::Uri;

use crate::lsp::config::LspConfig;
use crate::lsp::db::ArkDb;
use crate::lsp::open_file::OpenFile;
use crate::lsp::traits::url::UrlExt;

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
    /// The verbatim editor `Uri` is preserved on each [`OpenFile::wire_uri`]
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
    pub folders: Vec<AbsPathBuf>,
}

impl WorldState {
    pub(crate) fn new(db: OakDatabase) -> Self {
        Self {
            db,
            ..Default::default()
        }
    }

    /// Full read-only snapshot for a background reader that needs more than the
    /// db, e.g. completions read the workspace. Same shape as `self.clone()`,
    /// but the db is only reachable as `&dyn ArkDb`.
    pub(crate) fn snapshot(&self) -> WorldStateSnapshot {
        WorldStateSnapshot {
            db: self.db.clone(),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            config: self.config.clone(),
            workspace: self.workspace.clone(),
        }
    }

    /// Trimmed read-only snapshot for the diagnostics worker, which runs off
    /// the main loop and queries oak. Drops the workspace map the diagnostics
    /// pass doesn't read.
    pub(crate) fn diagnostics_snapshot(&self) -> WorldStateSnapshot {
        WorldStateSnapshot {
            db: self.db.clone(),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            config: self.config.clone(),
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

    pub(crate) fn open_file_mut(&mut self, path: &FilePath) -> anyhow::Result<&mut OpenFile> {
        self.open_files
            .get_mut(path)
            .ok_or_else(|| anyhow!("Can't find document for path {path}"))
    }

    /// The stored [`OpenFile`] for a request.
    pub(crate) fn open_file(&self, path: &FilePath) -> anyhow::Result<&OpenFile> {
        self.open_files
            .get(path)
            .ok_or_else(|| anyhow!("Can't find document for path {path}"))
    }

    /// The [`Uri`] to put on the wire for `file`. Open buffers replay the
    /// editor's verbatim `Uri`, so the frontend gets back exactly the bytes it
    /// sent us. Files that were never opened in the editor (disk-scanned
    /// files, resolution targets) have no verbatim `Uri`, so synthesise one
    /// from the normalised path.
    ///
    /// Fails only for a `Virtual` file that was never opened: `Url` tolerates
    /// characters in a path (`[`, `]`, `|`, `^`) that `Uri` rejects, so
    /// [`aether_path::VirtualUri`]'s stored `Url` can be one `Uri` can't parse.
    pub(crate) fn wire_uri(&self, file: File) -> anyhow::Result<Uri> {
        let path = file.path(&self.db);

        if let Some(open_file) = self.open_files.get(path) {
            return Ok(open_file.wire_uri().clone());
        }

        match path {
            FilePath::File(abs_path) => Uri::from_file_path(abs_path.as_path())
                .ok_or_else(|| anyhow!("error building a URI for path {abs_path}")),
            FilePath::Virtual(uri) => uri.as_url().to_uri(),
        }
    }

    /// Register an editor buffer in `open_files`, keying on `path` and stashing
    /// the verbatim editor `uri` on [`OpenFile::wire_uri`] for wire output.
    ///
    /// The caller is in charge of pushing the contents into `oak` via
    /// `upsert_editor()` and handing us the resulting [`File`].
    pub(crate) fn insert_open_file(
        &mut self,
        uri: Uri,
        path: FilePath,
        file: File,
        version: Option<i32>,
    ) {
        let open_file = OpenFile::new(file, version, uri);
        self.open_files.insert(path, open_file);
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
}

/// The wire `Uri` of every open buffer, paired with the [`FilePath`] it's keyed
/// on so a caller can look the buffer back up without converting.
pub(crate) fn open_file_wire_uris(state: &WorldState) -> Vec<(FilePath, Uri)> {
    state
        .open_files
        .iter()
        .map(|(path, open_file)| (path.clone(), open_file.wire_uri().clone()))
        .collect()
}
