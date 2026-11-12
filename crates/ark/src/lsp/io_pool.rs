//
// io_pool.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;

use crossbeam::channel::Sender;
use stdext::result::ResultExt;
use stdext::spawn_with_stack_size;

use crate::lsp;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::LspServiceContext;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::panic;
use crate::panic::Recovery;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A fixed set of OS threads running I/O jobs in FIFO order.
///
/// Jobs here must not own a salsa db handle. A download or an R subprocess
/// can't be interrupted by a Salsa cancellation. A handle sitting in this queue
/// would hold up the next main-loop write for that whole time.
pub(crate) struct IoPool {
    /// The pool's only sender. Drop it to disconnect the channel and shut down
    /// the workers.
    jobs_tx: Sender<Job>,

    service_context: Arc<LspServiceContext>,
}

impl IoPool {
    /// Start `threads` workers, each named `name` and given `stack_size` bytes
    /// of stack. Each lane picks its own size from the deepest call tree its
    /// jobs can reach, so use [`stdext::DEFAULT_STACK_SIZE`] unless you've
    /// bounded that.
    pub(crate) fn new(
        name: &'static str,
        threads: usize,
        stack_size: usize,
        service_context: Arc<LspServiceContext>,
    ) -> Self {
        let (jobs_tx, jobs_rx) = crossbeam::channel::unbounded::<Job>();

        for _ in 0..threads {
            let jobs_rx = jobs_rx.clone();
            let service_context = Arc::clone(&service_context);
            spawn_with_stack_size!(name, stack_size, move || {
                // `run_job()` recovers job panics. A panic that reaches this
                // boundary comes from the worker loop itself, so let this
                // worker exit rather than abort the process. The remaining
                // workers keep draining the queue.
                let outcome = panic::catch_unwind(Recovery::Always, || {
                    while let Ok(job) = jobs_rx.recv() {
                        run_job(job, &service_context);
                    }
                });
                if let Err(message) = outcome {
                    lsp::log_error!("Panic in an I/O worker: {message}");
                    service_context.report_background_panic();
                }
            });
        }

        Self {
            jobs_tx,
            service_context,
        }
    }

    /// Run `job` and send the event it returns to the main loop. If `job`
    /// panics, or never runs because every worker exited before reaching it,
    /// send `on_abort` instead. Every job sends exactly one event, so the
    /// scheduler that submitted it can always clear its in-flight state.
    ///
    /// After a panic, the panic is reported before `on_abort` is sent. The
    /// test harness checks for reported panics after handling each event, so
    /// it sees the report before `on_abort` lets the loop settle.
    ///
    /// Sending can race with main-loop shutdown. A failed send is only logged:
    /// once the receiver is gone, no scheduler remains to recover.
    pub(crate) fn submit_reporting(
        &self,
        events_tx: &TokioUnboundedSender<Event>,
        on_abort: Event,
        job: impl FnOnce() -> Event + Send + 'static,
    ) {
        self.submit(reporting_job(
            Arc::clone(&self.service_context),
            events_tx,
            on_abort,
            job,
        ));
    }

    /// Private so that every lane goes through [`Self::submit_reporting()`].
    fn submit(&self, job: impl FnOnce() + Send + 'static) {
        if self.jobs_tx.send(Box::new(job)).is_err() {
            lsp::log_error!("No live I/O worker left, dropping job");
        }
    }
}

fn run_job(job: Job, service_context: &LspServiceContext) {
    if let Err(message) = panic::catch_unwind(Recovery::Always, job) {
        lsp::log_error!("An I/O job panicked: {message}");
        service_context.report_background_panic();
    }
}

/// The job that [`IoPool::submit_reporting()`] queues. Catches `job`'s panic
/// itself rather than leaving it to [`run_job()`], so the panic is reported
/// before `on_abort` is sent.
fn reporting_job(
    service_context: Arc<LspServiceContext>,
    events_tx: &TokioUnboundedSender<Event>,
    on_abort: Event,
    job: impl FnOnce() -> Event + Send + 'static,
) -> impl FnOnce() + Send + 'static {
    let report = AbortReport {
        events_tx: events_tx.clone(),
        on_abort: Some(on_abort),
    };

    move || match panic::catch_unwind(Recovery::Always, job) {
        Ok(event) => report.complete(event),
        Err(message) => {
            lsp::log_error!("An I/O job panicked: {message}");
            service_context.report_background_panic();
            drop(report);
        },
    }
}

/// Sends `on_abort` when dropped, unless [`Self::complete()`] sent the job's
/// own event. Captured in the queued job, so dropping that job unrun sends
/// `on_abort` too.
struct AbortReport {
    events_tx: TokioUnboundedSender<Event>,
    on_abort: Option<Event>,
}

impl AbortReport {
    fn complete(mut self, event: Event) {
        self.on_abort = None;
        self.events_tx.send(event).log_err();
    }
}

impl Drop for AbortReport {
    fn drop(&mut self) {
        let Some(on_abort) = self.on_abort.take() else {
            return;
        };
        // Log with `log_err()`, not `lsp::log_error!`: the latter goes through
        // the auxiliary channel, which can panic. Queued jobs are dropped when
        // the last worker exits, possibly while it unwinds, and a panic in
        // `drop()` during unwinding aborts the process.
        self.events_tx.send(on_abort).log_err();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::main_loop::init_aux_for_test;
    use crate::lsp::main_loop::AuxiliaryEvent;
    use crate::lsp::main_loop::KernelNotification;
    use crate::lsp::state_handlers::ConsoleInputs;

    /// Install the production hook so a missing `catch_unwind()` aborts the
    /// process instead of silently losing the worker panic.
    #[test]
    fn test_pool_survives_panicking_job() {
        crate::panic::install();

        let context = Arc::new(LspServiceContext::new());
        let pool = IoPool::new("test-io-pool", 1, stdext::DEFAULT_STACK_SIZE, context);
        pool.submit(|| panic!("Test panic in an I/O job"));

        let (tx, rx) = std::sync::mpsc::channel();
        pool.submit(move || tx.send(()).unwrap());

        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    }

    /// Panics when dropped, so a job panicking with it as payload makes
    /// `run_job()` panic again when it drops the payload after its own
    /// `catch_unwind()` returns.
    struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("Test panic while dropping a job's panic payload");
        }
    }

    /// Install the production hook so a panic that escapes the worker's
    /// boundary aborts the test process. Two workers, so the one left after
    /// the other exits must still run jobs.
    #[test]
    fn test_pool_survives_panic_outside_a_job() {
        crate::panic::install();

        let context = Arc::new(LspServiceContext::new());
        let pool = IoPool::new(
            "test-io-pool",
            2,
            stdext::DEFAULT_STACK_SIZE,
            Arc::clone(&context),
        );
        pool.submit(|| std::panic::panic_any(PanicOnDrop));

        // The pool and each worker hold a clone of `context`. A worker drops
        // its clone when its thread returns through the boundary. An escaped
        // panic aborts before that.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while Arc::strong_count(&context) > 3 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let (tx, rx) = std::sync::mpsc::channel();
        pool.submit(move || tx.send(()).unwrap());

        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    }

    #[test]
    fn test_reporting_job_sends_only_its_event_when_run() {
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();

        reporting_job(context(), &events_tx, named("aborted"), || {
            named("completed")
        })();

        assert_eq!(name(events_rx.try_recv().unwrap()), "completed");
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn test_reporting_job_sends_abort_event_when_dropped() {
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();

        drop(reporting_job(
            context(),
            &events_tx,
            named("aborted"),
            || named("completed"),
        ));

        assert_eq!(name(events_rx.try_recv().unwrap()), "aborted");
        assert!(events_rx.try_recv().is_err());
    }

    /// Install the production hook so a panic that escapes `reporting_job()`'s
    /// own `catch_unwind()` aborts the test process.
    #[test]
    fn test_reporting_job_reports_panic_and_sends_abort_event() {
        crate::panic::install();
        let mut auxiliary_rx = init_aux_for_test();
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();

        reporting_job(context(), &events_tx, named("aborted"), || {
            panic!("Test panic in a reporting job")
        })();

        assert!(matches!(
            auxiliary_rx.try_recv(),
            Ok(AuxiliaryEvent::ReportBackgroundPanic)
        ));
        assert_eq!(name(events_rx.try_recv().unwrap()), "aborted");
        assert!(events_rx.try_recv().is_err());
    }

    /// With no workers the channel has no receiver, so `submit()` drops the
    /// job unrun.
    #[test]
    fn test_submit_reporting_sends_abort_event_without_workers() {
        let pool = IoPool::new("test-io-pool", 0, stdext::DEFAULT_STACK_SIZE, context());
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();

        pool.submit_reporting(&events_tx, named("aborted"), || named("completed"));

        assert_eq!(name(events_rx.try_recv().unwrap()), "aborted");
        assert!(events_rx.try_recv().is_err());
    }

    fn context() -> Arc<LspServiceContext> {
        Arc::new(LspServiceContext::new())
    }

    /// An event that carries `name`, so tests can tell which event was sent.
    fn named(name: &str) -> Event {
        Event::Kernel(KernelNotification::DidChangeConsoleInputs(ConsoleInputs {
            console_scopes: vec![vec![String::from(name)]],
        }))
    }

    fn name(event: Event) -> String {
        let Event::Kernel(KernelNotification::DidChangeConsoleInputs(inputs)) = event else {
            panic!("Expected a test event, got {event:?}");
        };
        inputs.console_scopes[0][0].clone()
    }
}
