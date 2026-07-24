//
// warmup.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use oak_db::all_used_files;
use oak_db::warm_file;

use super::pool::AnalysisPool;
use crate::lsp;
use crate::lsp::indexer;
use crate::lsp::state::WorldState;

/// Build the per-file workspace symbol indexes on a background thread so
/// main-loop consumers triggered by the user (workspace symbols, workspace
/// completions) find them already computed. The first run after a workspace
/// scan does the real work, parsing and walking each file. Later runs only
/// revalidate the per-file memos.
///
/// Mirrors rust-analyzer's cache warming: spawned when a workspace scan
/// settles, the analogue of r-a's transitions to quiescence (initial VFS scan,
/// workspace reload, etc). Unlike r-a we don't restart a warmup that gets
/// cancelled (the pool swallows the unwind). A cancelling write can only come
/// from an editor buffer, so a document is open, and the diagnostics passes
/// spawned by that same write force the same memos and finish the job.
pub(crate) fn warm_workspace_index(state: &WorldState, pool: &AnalysisPool) {
    pool.spawn(state.snapshot(), |snapshot| {
        let now = std::time::Instant::now();
        lsp::log_info!("Starting workspace index warmup");
        indexer::warm(snapshot.db());
        lsp::log_info!("Finished workspace index warmup ({:.0?})", now.elapsed());
    })
}

/// Warm the oak `semantic_index` of every file the workspace depends on, on a
/// background thread.
///
/// Idempotent, so re-running on every revision is cheap once a file's index is
/// already warm (salsa cache hit). A concurrent write just cancels the
/// in-flight warm; the next revision re-runs it, which is what carries warmup
/// through the startup write-storm and warms a freshly-typed `pkg::`
/// dependency as soon as its sources land.
pub(crate) fn warm_semantic_indexes(state: &WorldState, pool: &AnalysisPool) {
    pool.spawn(state.snapshot(), |snapshot| {
        let now = std::time::Instant::now();
        for &file in all_used_files(snapshot.db()) {
            warm_file(snapshot.db(), file);
        }
        lsp::log_info!("Warmed semantic indexes ({:.0?})", now.elapsed());
    })
}
