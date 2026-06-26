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
//! Instead of watching the clock from another thread, we piggyback on R's
//! interrupt-time hook, `Console::interrupt_events()`, and call
//! [`check_timeout()`] from there. R runs that hook whenever it checks for a
//! user interrupt, so a runaway `f` that polls for interrupts also gives us a
//! chance to notice its deadline has passed. When it has, we ask R to interrupt
//! itself. The interrupt unwinds R back to the nearest `try_catch()`, which
//! surfaces it as an error.
//!
//! The hook fires on both platforms (Unix via `R_PolledEvents`, Windows via the
//! `R_ProcessEvents` callback), so the timeout works everywhere.

use std::cell::Cell;
use std::time::Duration;
use std::time::Instant;

use harp::raii::RLocalInterruptsSuspended;

/// How long an evaluation in `with_timeout()` may run before we interrupt it.
pub(crate) const EVAL_TIMEOUT: Duration = Duration::from_secs(1);

// These variables are thread-local to provide safe lock-free interior
// mutability on the R thread
thread_local! {
    /// When the in-flight evaluation should be interrupted, if it's still
    /// running. Set by `with_timeout()`, read by `check_timeout()`.
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };

    /// Set by `check_timeout()` when it trips the interrupt, read by
    /// `with_timeout()` which reports the timeout.
    static TIMED_OUT: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` on the R thread, interrupting it if it outlasts `timeout`.
///
/// `f` runs in `try_catch()` with interrupts re-enabled so the interrupt that
/// `check_timeout()` trips can fire and longjump. The interrupt-time hook that
/// calls `check_timeout()` is installed for the whole session, so unlike
/// interrupts it needs no fiddling here.
///
/// Note that the longjump is either caught by our `try_catch()`, or a
/// `try_catch()` inside `f`. An alternative approach would be to try and
/// propagate the interrupt with a Rust panic, but that needs to be carefully
/// engineered. Be aware that because of the approach taken here, it's possible
/// for code inside `f` to try and recover from Rust errors
/// (`harp::Error::TopLevelExecError`) caused by the cancellation interrupt.
///
/// Returns `f`'s value wrapped in the `try_catch()` result and a boolean
/// indicating whether the timeout fired.
///
/// Must run on the R thread.
pub(crate) fn with_timeout<F, T>(timeout: Duration, f: F) -> (harp::Result<T>, bool)
where
    F: FnOnce() -> T,
{
    // Save and restore so a nested evaluation doesn't clobber an outer deadline.
    let old_deadline = DEADLINE.replace(Some(Instant::now() + timeout));
    let old_timed_out = TIMED_OUT.replace(false);

    let res = try_catch_with_timeout(f);

    let timed_out = TIMED_OUT.get();

    // If `check_timeout()` tripped the interrupt but `f` finished before R
    // acted on it, a stray interrupt is left pending. Clear it here so it can't
    // fire on a later evaluation.
    if timed_out {
        crate::signals::set_interrupts_pending(false);
    }

    DEADLINE.set(old_deadline);
    TIMED_OUT.set(old_timed_out);

    (res, timed_out)
}

/// Called from the polled-events handler at R's interrupt check points. If the
/// in-flight evaluation has outlived its deadline, ask R to interrupt itself.
/// Runs on the R thread.
pub(crate) fn check_timeout() {
    if TIMED_OUT.get() {
        // Already tripped, don't ask twice
        return;
    }
    let Some(deadline) = DEADLINE.get() else {
        // No evaluation under a timeout
        return;
    };
    if Instant::now() >= deadline {
        TIMED_OUT.set(true);
        crate::signals::set_interrupts_pending(true);
    }
}

/// Run `f` in a `try_catch` with interrupts live.
///
/// The event loop (and ReadConsole) run with interrupts suspended. We re-enable
/// them so the interrupt that `check_timeout()` trips can fire and longjump out
/// of `f`. The interrupt-time hook that calls `check_timeout()` is installed for
/// the whole session, so unlike interrupts it needs no re-enabling.
fn try_catch_with_timeout<F, T>(f: F) -> harp::Result<T>
where
    F: FnOnce() -> T,
{
    let _interrupts = RLocalInterruptsSuspended::new(false);
    harp::try_catch(f)
}
