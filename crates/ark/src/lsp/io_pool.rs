//
// io_pool.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::panic::AssertUnwindSafe;

use crossbeam::channel::Sender;
use stdext::panic_message;
use stdext::spawn_with_stack;

use crate::lsp;

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
    pub(crate) fn new(name: &'static str, threads: usize, stack_size: usize) -> Self {
        let (jobs_tx, jobs_rx) = crossbeam::channel::unbounded::<Job>();

        for _ in 0..threads {
            let jobs_rx = jobs_rx.clone();
            spawn_with_stack!(name, stack_size, move || {
                while let Ok(job) = jobs_rx.recv() {
                    run_job(job);
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

fn run_job(job: Job) {
    if let Err(err) = std::panic::catch_unwind(AssertUnwindSafe(job)) {
        lsp::log_error!(
            "An I/O job panicked: {msg}",
            msg = panic_message(err.as_ref())
        );
    }
}
