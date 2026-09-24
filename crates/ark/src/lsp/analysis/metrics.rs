//! Session counters for background analysis, kept separate from diagnostics
//! scheduling state so the main loop's idle check does not depend on
//! instrumentation.

use super::pool::AnalysisPool;
use crate::lsp;

/// Session counters for diagnostics scheduling.
///
/// The main loop is the only writer of these counters. Worker threads update
/// [`super::pool::PoolMetrics`] without synchronizing with the main loop, so a
/// [`Self::log_snapshot`] taken while tasks are in flight is not a settled total.
#[derive(Default, Debug, Clone, Copy)]
pub struct DiagnosticsMetrics {
    pub batches: u64,
    pub tasks_queued: u64,
    /// Results the main loop accepted as the newest generation for their file.
    /// [`crate::lsp::publish_diagnostics`] can still skip the client notification
    /// when the diagnostic set is unchanged.
    pub results_accepted: u64,
    /// Results discarded because a newer generation was published for the same file.
    pub results_stale: u64,
}

impl DiagnosticsMetrics {
    pub(crate) fn record_batch(&mut self, tasks: u64) {
        self.batches += 1;
        self.tasks_queued += tasks;
    }

    pub(crate) fn record_accepted(&mut self) {
        self.results_accepted += 1;
    }

    pub(crate) fn record_stale(&mut self) {
        self.results_stale += 1;
    }

    /// Tasks without an accepted or stale result in this snapshot. This includes
    /// queued or running tasks, tasks replaced by a newer keyed task, and tasks
    /// cancelled or panicked before completing. Unlike
    /// [`super::pool::PoolMetrics`], this does not distinguish those outcomes.
    pub(crate) fn results_not_returned(&self) -> i64 {
        self.tasks_queued as i64 - self.results_accepted as i64 - self.results_stale as i64
    }

    /// Logs diagnostic and pool counters at trace level. Worker-thread counters
    /// can change while this runs, so this is a snapshot rather than a settled queue.
    pub(crate) fn log_snapshot(&self, pool: &AnalysisPool) {
        // Avoid acquiring the `AnalysisPool` metrics lock when trace output is
        // disabled. `lsp::log_trace!()` also checks before formatting, but
        // `log_snapshot()` runs for every `DiagnosticsReady` event and refresh batch.
        if !lsp::trace_enabled!() {
            return;
        }

        let queue = pool.metrics();

        lsp::log_trace!(
            "Diagnostics snapshot: {batches} batches, {tasks_queued} tasks = {results_accepted} accepted + {results_stale} stale + {results_not_returned} not returned. \
             Analysis queue: {queued} queued = {replaced} replaced + {cancelled_queued} dropped before start + {started} started + {waiting} waiting. \
             Of the started: {completed} completed + {cancelled_running} cancelled mid-pass + {panicked} panicked + {running} running. \
             Peak depth {peak_queue_len}",
            batches = self.batches,
            tasks_queued = self.tasks_queued,
            results_accepted = self.results_accepted,
            results_stale = self.results_stale,
            results_not_returned = self.results_not_returned(),
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
}

#[cfg(test)]
mod tests {
    use super::DiagnosticsMetrics;

    #[test]
    fn test_diagnostics_metrics_accounting() {
        let mut metrics = DiagnosticsMetrics::default();

        metrics.record_batch(3);
        assert_eq!(metrics.batches, 1);
        assert_eq!(metrics.tasks_queued, 3);
        assert_eq!(metrics.results_not_returned(), 3);

        metrics.record_accepted();
        metrics.record_accepted();
        metrics.record_stale();
        assert_eq!(metrics.results_accepted, 2);
        assert_eq!(metrics.results_stale, 1);
        assert_eq!(metrics.results_not_returned(), 0);

        // A second batch adds to the running totals rather than replacing them.
        metrics.record_batch(2);
        assert_eq!(metrics.batches, 2);
        assert_eq!(metrics.tasks_queued, 5);
        assert_eq!(metrics.results_not_returned(), 2);
    }
}
