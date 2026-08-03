//
// refresh.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;

use aether_path::FilePath;
use stdext::result::ResultExt;

use super::pool::AnalysisPool;
use super::snapshot::WorldStateSnapshot;
use crate::lsp;
use crate::lsp::diagnostics::generate_diagnostics;
use crate::lsp::main_loop::DiagnosticsPublication;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::open_file::OpenFile;
use crate::lsp::state::WorldState;
use crate::url::FilePathExt;

/// A diagnostics task's result on its way back to the main loop. The generation
/// state enables [`DiagnosticsState::accept`] to distinguish a stale result
/// from a fresh one.
#[derive(Debug)]
pub(crate) struct DiagnosticsReady {
    pub(crate) generation: u64,
    pub(crate) publication: DiagnosticsPublication,
}

/// Tracks diagnostics staleness across refresh batches, so an out-of-order
/// result gets dropped instead of published over a newer one.
///
/// Mirrors rust-analyzer's generation counter in
/// `crates/rust-analyzer/src/diagnostics.rs`.
#[derive(Default)]
pub(crate) struct DiagnosticsState {
    /// Bumped once per refresh batch.
    generation: u64,
    /// Generation of the newest result published per file.
    published: HashMap<FilePath, u64>,
}

impl DiagnosticsState {
    /// Queue a diagnostics pass for every open file we diagnose, all tagged
    /// with a new generation.
    pub(crate) fn refresh_all(
        &mut self,
        state: &WorldState,
        pool: &AnalysisPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        self.generation += 1;
        let generation = self.generation;

        let files: Vec<(&FilePath, &OpenFile)> = state
            .open_files
            .iter()
            .filter(|(path, _open_file)| path.should_diagnose())
            .collect();

        tracing::trace!("Refreshing diagnostics for {n} documents", n = files.len());
        lsp::log_info!("Queueing {n} diagnostic tasks", n = files.len());

        for (path, open_file) in files {
            let path = path.clone();
            let file = open_file.clone();
            let events_tx = events_tx.clone();

            pool.spawn_keyed(path.clone(), state.snapshot(), move |snapshot| {
                let publication = refresh_diagnostics(path, file, snapshot);
                let ready = DiagnosticsReady {
                    generation,
                    publication,
                };
                events_tx.send(Event::DiagnosticsReady(ready)).log_err();
            });
        }
    }

    /// Whether a diagnostics result for `path` computed at `generation`
    /// should be published now, or is stale and should be dropped.
    ///
    /// Equal generations can't legitimately arrive twice for the same file:
    /// we spawn one task per file per batch, and keyed replacement on the
    /// pool keeps at most one queued entry per file.
    pub(crate) fn accept(&mut self, path: &FilePath, generation: u64) -> bool {
        if let Some(published) = self.published.get(path) {
            if *published > generation {
                return false;
            }
        }

        self.published.insert(path.clone(), generation);
        true
    }

    /// Generation of the newest result already published for `path`, for the
    /// main loop to log alongside a dropped stale result.
    pub(crate) fn published_generation(&self, path: &FilePath) -> Option<u64> {
        self.published.get(path).copied()
    }
}

fn refresh_diagnostics(
    path: FilePath,
    file: OpenFile,
    state: WorldStateSnapshot,
) -> DiagnosticsPublication {
    let uri = file.wire_uri().clone();
    let version = file.version();
    let _span = tracing::info_span!("diagnostics_refresh", uri = %uri.as_str()).entered();

    // Special case testthat-specific behaviour. This is a simple stopgap
    // approach that has some false positives (e.g. when we work on testthat
    // itself the flag will always be true), but that shouldn't have much
    // practical impact.
    let testthat = path
        .as_path()
        .is_some_and(|path| path.components().any(|c| c.as_str() == "testthat"));

    let now = std::time::Instant::now();
    lsp::log_info!("Generating diagnostics for file: {}", uri.as_str());

    let diagnostics = generate_diagnostics(file.file(), state, testthat, &uri);

    lsp::log_info!(
        "Finished diagnostics for file: {} in {:.0?}",
        uri.as_str(),
        now.elapsed()
    );

    DiagnosticsPublication {
        path,
        uri,
        diagnostics,
        version,
    }
}

#[cfg(test)]
mod tests {
    use aether_path::FilePath;
    use oak_scan::DbScan;
    use url::Url;

    use super::refresh_diagnostics;
    use super::DiagnosticsState;
    use crate::lsp::analysis::catch_cancellation;
    use crate::lsp::state::WorldState;
    use crate::lsp::traits::url::UrlExt;

    /// `accept` is the staleness gate a refresh batch relies on: a result only
    /// publishes if no newer generation for that file already went out. Pins
    /// the three cases that can arrive at the main loop: a file seen for the
    /// first time, a fresh batch superseding the last one, and a straggler
    /// from an old batch arriving after a newer one already landed. Also pins
    /// the deliberate choice to accept a repeat of the last generation (see
    /// `accept`'s doc comment for why that can't happen in practice but is
    /// still safe).
    #[test]
    fn test_accept_tracks_staleness_per_file() {
        let mut diagnostics = DiagnosticsState::default();
        let path = FilePath::from_url(&Url::parse("file:///test.R").unwrap());

        assert!(diagnostics.accept(&path, 1));
        assert!(diagnostics.accept(&path, 2));
        assert!(!diagnostics.accept(&path, 1));
        assert!(diagnostics.accept(&path, 2));
    }

    /// A salsa cancellation during the pass is swallowed into `None` by
    /// `catch_cancellation`, the wrapper the pool applies to every task, rather
    /// than unwinding and killing the worker thread.
    ///
    /// `cancellation_token().cancel()` arms local cancellation on the snapshot's
    /// oak, so the first salsa query in `generate_diagnostics` (the `tree_sitter`
    /// fetch) unwinds with `salsa::Cancelled`, the same payload a concurrent
    /// `set_*` produces. The unwind fires before any R, so no `r_task` here.
    #[test]
    fn test_cancelled_diagnostics_pass_is_caught() {
        let mut state = WorldState::default();
        let uri = Url::parse("file:///test.R").unwrap();
        let path = FilePath::from_url(&uri);
        let code = "foo";
        let file = state.db.upsert_editor(path.clone(), code.to_string());
        state.insert_open_file(uri.to_uri().unwrap(), path.clone(), file, None);

        let file = state.open_file(&path).unwrap().clone();
        let snapshot = state.snapshot();
        snapshot.cancellation_token().cancel();

        assert!(catch_cancellation(|| refresh_diagnostics(path, file, snapshot)).is_none());
    }
}
