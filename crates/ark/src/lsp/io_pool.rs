//
// io_pool.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;

use crossbeam::channel::Sender;
use stdext::spawn_with_stack_size;

use crate::lsp;
use crate::lsp::main_loop::LspServiceContext;
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

        Self { jobs_tx }
    }

    pub(crate) fn submit(&self, job: impl FnOnce() + Send + 'static) {
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

#[cfg(test)]
mod tests {
    use super::*;

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

        // Each worker holds a clone of `context` and drops it when its thread
        // returns through the boundary. An escaped panic aborts before that.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while Arc::strong_count(&context) > 2 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let (tx, rx) = std::sync::mpsc::channel();
        pool.submit(move || tx.send(()).unwrap());

        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    }
}
