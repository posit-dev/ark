use std::collections::HashMap;
use std::path::Path;

use aether_path::FilePath;
use anyhow::anyhow;
use oak_db::OakDatabase;
use oak_semantic::library::Library;
use url::Url;

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

    /// Copy the world state for a background handler that does not query oak.
    ///
    /// The copy gets a fresh, empty `OakDatabase` instead of a handle to the
    /// live one. A salsa db handle held off the main loop blocks the next
    /// `set_*` on the owner: the setter waits for `clones == 1`, and an idle
    /// handle (parked in the indexer queue, or held by a handler blocked in
    /// `r_task`) never drops on its own.
    ///
    /// This is the snapshot for the non-salsa handlers (diagnostics,
    /// indexing) that read only the plain `WorldState` fields. A salsa-based
    /// handler that queries oak off the main loop needs a different snapshot,
    /// one that keeps the live db handle and runs its queries under
    /// cancellation (catch `Cancelled`, don't span `r_task`), so it sees real
    /// oak data. That's what the `legacy_` prefix warns: don't reach for this
    /// from oak-querying code.
    pub(crate) fn legacy_snapshot(&self) -> WorldState {
        WorldState {
            db: OakDatabase::new(),
            ..self.clone()
        }
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
    let Some(key) = FilePath::from_path_buf(path.to_path_buf()) else {
        log::info!(
            "couldn't construct file path from {}; reading from disk instead",
            path.display()
        );
        return fallback();
    };

    let Ok(document) = state.get_document(&key) else {
        log::info!("no document for path {key}; reading from disk instead");
        return fallback();
    };

    callback(document)
}

pub(crate) fn workspace_uris(state: &WorldState) -> Vec<Url> {
    state
        .documents
        .values()
        .map(|doc| doc.url.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::time::Duration;

    use oak_db::OakDatabase;
    use oak_scan::DbScan;
    use oak_semantic::library::Library;

    use super::WorldState;

    /// A legacy background snapshot must not pin the oak db against a
    /// main-loop mutation.
    ///
    /// salsa reclaims `&mut` access for a setter by raising the cancellation
    /// flag and then blocking on `clones == 1`. That flag only frees a clone
    /// whose thread is inside a running query and notices it. A snapshot that
    /// sits idle (parked in the indexer queue, or held by a `spawn_blocking`
    /// handler blocked in `r_task`) never notices, so the next setter on the
    /// owner blocks until the snapshot drops. This test parks a snapshot with
    /// no query running and asserts a setter on the owner still completes.
    #[test]
    fn legacy_snapshot_does_not_pin_oak_against_mutation() {
        let mut state = WorldState::new(OakDatabase::new(), Library::new(vec![]));

        let snapshot = state.legacy_snapshot();

        // Park the snapshot with no salsa query running, then hold it until
        // the main thread has finished timing the mutation.
        let release = Arc::new(Barrier::new(2));
        let held = {
            let release = Arc::clone(&release);
            std::thread::spawn(move || {
                let _snapshot = snapshot;
                release.wait();
            })
        };

        let (tx, rx) = mpsc::channel();
        let mutator = std::thread::spawn(move || {
            state.db.set_library_paths(&[]);
            let _ = tx.send(());
        });

        let completed = rx.recv_timeout(Duration::from_secs(2)).is_ok();

        // Release the parked snapshot so a blocked mutator can finish and both
        // threads join, regardless of the outcome.
        release.wait();
        held.join().unwrap();
        mutator.join().unwrap();

        assert!(completed);
    }
}
