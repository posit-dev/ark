//
// watchdog.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Detects a main-loop tick that arms and never disarms, usually a Salsa write
//! parked behind a background task that can't drop its snapshot.
//! `handle_event()` in `main_loop.rs` arms one [`TickGuard`] per tick, and the
//! poller thread here reports a tick still armed past the deadline.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use stdext::spawn;

use crate::lsp;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// `armed_at` sentinel indicating that no tick is currently running.
const IDLE: u64 = u64::MAX;

/// A tick is allowed to run this long before we consider it stuck. Production
/// gets more slack because a tick can legitimately await a client round-trip
/// (`did_change_configuration` calls out to the client) or apply a large
/// workspace scan.
fn deadline() -> Duration {
    if stdext::IS_TESTING {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(30)
    }
}

/// Watches for a main-loop tick that never disarms. Owns one poller thread for
/// the lifetime of the session; dropping the watchdog closes it down within
/// one poll interval.
pub(crate) struct Watchdog {
    shared: Arc<Shared>,
}

struct Shared {
    /// Millis since `start` when the current tick armed, or [`IDLE`].
    armed_at: AtomicU64,
    /// Bumped on each arm so the poller reports a given stuck tick once.
    tick: AtomicU64,
    /// `outstanding_holds()` as of the arm.
    holds: AtomicUsize,
    closed: AtomicBool,
    start: Instant,
}

impl Watchdog {
    pub(crate) fn new() -> Self {
        let shared = Arc::new(Shared {
            armed_at: AtomicU64::new(IDLE),
            tick: AtomicU64::new(0),
            holds: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            start: Instant::now(),
        });

        let poller = Arc::clone(&shared);
        spawn!("oak-watchdog", move || poll(poller));

        Self { shared }
    }

    /// Arm the watchdog for one tick. Dropping the returned guard disarms it
    /// again, on every exit path out of `handle_event` including `?` early
    /// returns and panics.
    pub(crate) fn tick(&self, holds: usize) -> TickGuard {
        self.shared.tick.fetch_add(1, Ordering::SeqCst);
        self.shared.holds.store(holds, Ordering::SeqCst);
        let armed_at = self.shared.start.elapsed().as_millis() as u64;
        self.shared.armed_at.store(armed_at, Ordering::SeqCst);

        TickGuard {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
    }
}

/// Owns the arm for one main-loop tick.
pub(crate) struct TickGuard {
    shared: Arc<Shared>,
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        self.shared.armed_at.store(IDLE, Ordering::SeqCst);
    }
}

/// Poll `shared` until closed, reporting a tick that's been armed longer than
/// [`deadline()`]. Reports a given tick once when it first crosses the
/// deadline, then again roughly every 30s while it's still armed, so the log
/// shows the stall is ongoing rather than a single orphaned line.
fn poll(shared: Arc<Shared>) {
    let deadline = deadline();
    let mut reported_tick = 0;
    let mut last_report_at = Duration::ZERO;

    loop {
        std::thread::sleep(POLL_INTERVAL);
        if shared.closed.load(Ordering::SeqCst) {
            return;
        }

        let armed_at = shared.armed_at.load(Ordering::SeqCst);
        if armed_at == IDLE {
            continue;
        }
        let armed_at = Duration::from_millis(armed_at);

        // `armed_at` is a truncated offset from `start`, so it can't be later
        // than what `elapsed()` reads now.
        let elapsed = shared.start.elapsed() - armed_at;
        if elapsed <= deadline {
            continue;
        }

        let tick = shared.tick.load(Ordering::SeqCst);
        let is_new_stall = tick != reported_tick;
        let due_for_rereport = elapsed.saturating_sub(last_report_at) >= REPORT_INTERVAL;
        if !is_new_stall && !due_for_rereport {
            continue;
        }

        reported_tick = tick;
        last_report_at = elapsed;
        report(tick, elapsed, shared.holds.load(Ordering::SeqCst));
    }
}

fn report(tick: u64, elapsed: Duration, holds: usize) {
    let message = format!(
        "Main loop tick {tick} has been running for {secs:.1}s with {holds} outstanding Salsa \
         db holds. Likely cause: a write parked waiting for a background reader to drop its \
         snapshot.",
        secs = elapsed.as_secs_f64(),
    );

    if stdext::IS_TESTING {
        // The stuck thread is the main loop, so panicking here would only unwind this
        // poller thread and the test would still hang to the harness timeout with no
        // information. nextest runs each test in its own process, so aborting attributes
        // the failure to the right test and prints the diagnosis.
        eprintln!("{message}");
        std::process::abort();
    } else {
        lsp::log_error!("{message}");
        log::error!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::Watchdog;
    use super::IDLE;

    #[test]
    fn test_tick_arms_and_disarms() {
        let watchdog = Watchdog::new();
        assert_eq!(watchdog.shared.armed_at.load(Ordering::SeqCst), IDLE);

        let guard = watchdog.tick(3);
        assert_ne!(watchdog.shared.armed_at.load(Ordering::SeqCst), IDLE);
        assert_eq!(watchdog.shared.holds.load(Ordering::SeqCst), 3);

        drop(guard);
        assert_eq!(watchdog.shared.armed_at.load(Ordering::SeqCst), IDLE);
    }

    #[test]
    fn test_consecutive_ticks_increase() {
        let watchdog = Watchdog::new();

        let before = watchdog.shared.tick.load(Ordering::SeqCst);
        let _first = watchdog.tick(0);
        let after_first = watchdog.shared.tick.load(Ordering::SeqCst);
        let _second = watchdog.tick(0);
        let after_second = watchdog.shared.tick.load(Ordering::SeqCst);

        assert!(after_first > before);
        assert!(after_second > after_first);
    }
}
