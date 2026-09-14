//! Session counters for background analysis, kept separate from diagnostics
//! scheduling state so the main loop's idle check does not depend on
//! instrumentation.

use super::pool::AnalysisPool;
use super::refresh::DiagnosticsState;
use crate::lsp;

/// Session counters for diagnostics scheduling.
///
/// The main loop is the only writer. Analysis threads update
/// [`super::pool::PoolMetrics`] directly.
#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct DiagnosticsMetrics {
    pub(crate) batches: u64,
    pub(crate) tasks_queued: u64,
    pub(crate) results_published: u64,
    /// Results discarded because a newer generation was published for the same file.
    pub(crate) results_stale: u64,
    /// Generation whose settled counters were logged. Prevents late results
    /// from logging the same batch again.
    reported_batch: u64,
}

impl DiagnosticsMetrics {
    pub(crate) fn record_batch(&mut self, tasks: u64) {
        self.batches += 1;
        self.tasks_queued += tasks;
    }

    pub(crate) fn record_published(&mut self) {
        self.results_published += 1;
    }

    pub(crate) fn record_stale(&mut self) {
        self.results_stale += 1;
    }

    /// Tasks replaced or cancelled before they published or became stale. A
    /// negative value signals unbalanced counters.
    pub(crate) fn without_result(&self) -> i64 {
        self.tasks_queued as i64 - self.results_published as i64 - self.results_stale as i64
    }
}

pub(crate) fn log_settled(
    metrics: &mut DiagnosticsMetrics,
    diagnostics: &DiagnosticsState,
    pool: &AnalysisPool,
) {
    if diagnostics.pending_in_batch() > 0 || metrics.reported_batch == diagnostics.generation() {
        return;
    }
    metrics.reported_batch = diagnostics.generation();

    let queue = pool.metrics();

    lsp::log_info!(
        "Diagnostics since startup: {batches} batches, {tasks_queued} tasks = {results_published} published + {results_stale} stale + {without_result} without a result. \
         Analysis queue: {queued} queued = {replaced} replaced + {cancelled_queued} dropped before start + {started} started + {waiting} waiting. \
         Of the started: {completed} completed + {cancelled_running} cancelled mid-pass + {panicked} panicked + {running} running. \
         Peak depth {peak_queue_len}",
        batches = metrics.batches,
        tasks_queued = metrics.tasks_queued,
        results_published = metrics.results_published,
        results_stale = metrics.results_stale,
        without_result = metrics.without_result(),
        queued = queue.queued,
        replaced = queue.replaced,
        cancelled_queued = queue.cancelled_queued,
        started = queue.started,
        waiting = queue.waiting(),
        completed = queue.completed,
        cancelled_running = queue.cancelled_running,
        panicked = queue.panicked,
        running = queue.running(),
        peak_queue_len = queue.peak_queue_len,
    );
}
