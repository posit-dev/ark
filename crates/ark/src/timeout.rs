//
// timeout.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! A timeout that breaks runaway R-thread work with an interrupt.
//!
//! R runs on a single thread, so work handed to it (a debugger `evaluate`, say)
//! can loop forever and freeze the kernel. There's no way to cancel R code from
//! the outside other than an interrupt, which R polls for at loop iterations and
//! other check points.
//!
//! [`InterruptTimeout`] pairs a waiter on the spawning thread with a runner on
//! the R thread. The runner executes the work in an interruptible sandbox and
//! signals when it's done. The waiter blocks on that signal, and if the work
//! outlasts the timeout it asks R to interrupt itself (SIGINT on Unix,
//! `UserBreak` on Windows). The interrupt unwinds R back to the nearest
//! `try_catch`, which surfaces it as an error.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crossbeam::channel::bounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::RecvTimeoutError;
use crossbeam::channel::Sender;
use harp::exec::r_sandbox_interruptible;
use stdext::result::ResultExt;

type SharedOption<T> = Arc<Mutex<Option<T>>>;

/// The spawning-thread half of the timeout. See the [module docs](self).
///
/// Create the pair with [`InterruptTimeout::new`], move the [`InterruptRunner`]
/// onto the R thread and call [`InterruptRunner::run`] there, and call
/// [`InterruptTimeout::wait`] on the spawning thread.
pub(crate) struct InterruptTimeout<T> {
    timeout: Duration,
    done_rx: Receiver<()>,
    result: SharedOption<harp::Result<T>>,
    /// Shared with the runner so it can tell whether we asked for an interrupt
    /// and clear a stray one. An `Arc` rather than a global keeps it scoped to
    /// this one task.
    requested: Arc<AtomicBool>,
}

/// The R-thread half of an [`InterruptTimeout`].
pub(crate) struct InterruptRunner<T> {
    done_tx: Sender<()>,
    result: SharedOption<harp::Result<T>>,
    requested: Arc<AtomicBool>,
}

pub(crate) enum InterruptOutcome<T> {
    /// The work signalled completion within the timeout.
    Completed(harp::Result<T>),
    /// The work outlasted the timeout and was interrupted.
    TimedOut,
    /// The runner went away without signalling (e.g. the event loop shut down).
    Disconnected,
}

impl<T> InterruptTimeout<T> {
    pub(crate) fn new(timeout: Duration) -> (Self, InterruptRunner<T>) {
        let result: SharedOption<harp::Result<T>> = Arc::new(Mutex::new(None));
        let requested = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = bounded::<()>(1);

        let waiter = Self {
            timeout,
            done_rx,
            result: Arc::clone(&result),
            requested: Arc::clone(&requested),
        };
        let runner = InterruptRunner {
            done_tx,
            result,
            requested,
        };
        (waiter, runner)
    }

    /// Block until the runner signals completion. If that takes longer than the
    /// timeout, ask R to interrupt itself and wait for the resulting unwind.
    ///
    /// We trust the timeout to label the outcome rather than inspecting the
    /// result: an inner `try_catch` (e.g. in `parse_eval0`) often catches the
    /// interrupt and turns it into an ordinary error, so the result alone can't
    /// tell a timeout apart from a regular failure.
    pub(crate) fn wait(self) -> InterruptOutcome<T> {
        match self.done_rx.recv_timeout(self.timeout) {
            Ok(()) => {},
            Err(RecvTimeoutError::Timeout) => {
                // Set the flag before requesting the interrupt so the runner can
                // recognise our request and clean up after it.
                self.requested.store(true, Ordering::SeqCst);
                crate::sys::control::handle_interrupt_request();
                if self.done_rx.recv().log_err().is_none() {
                    return InterruptOutcome::Disconnected;
                }
                return InterruptOutcome::TimedOut;
            },
            Err(RecvTimeoutError::Disconnected) => return InterruptOutcome::Disconnected,
        }

        let out = self.result.lock().unwrap().take();
        match out {
            Some(res) => InterruptOutcome::Completed(res),
            None => InterruptOutcome::Disconnected,
        }
    }
}

impl<T> InterruptRunner<T> {
    /// Run `f` interruptibly on the R thread, store its result, and signal the
    /// waiter. Must be called on the R thread.
    pub(crate) fn run<F>(self, f: F)
    where
        F: FnOnce() -> T,
    {
        // Run `f` in an interruptible sandbox so an R error or a timeout
        // interrupt longjumps back to here rather than unwinding past this
        // frame. The `done` signal must stay outside the sandbox: a longjump
        // skips everything between the error and the sandbox `setjmp`, so
        // signalling inside would strand the waiter forever in `recv()`.
        let res = r_sandbox_interruptible(f);

        // If the waiter asked for a timeout interrupt but `f` finished just
        // before R acted on it, a stray interrupt is left pending. Clear it
        // here, on the R thread (the only safe place to touch it), so it can't
        // fire on a later evaluation.
        if self.requested.swap(false, Ordering::SeqCst) {
            crate::signals::set_interrupts_pending(false);
        }

        *self.result.lock().unwrap() = Some(res);
        let _ = self.done_tx.send(());
    }
}
