//
// warmup.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

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
