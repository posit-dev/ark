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

use std::time::Duration;
use std::time::Instant;

use crossbeam::channel::select;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use stdext::result::ResultExt;
use stdext::spawn_with_stack_size;

use crate::lsp;

const DEADLINE: Duration = Duration::from_secs(5);

/// A tick arming or disarming, carried from `Watchdog`/`TickGuard` to the
/// poller thread.
enum Tick {
    Armed {
        tick: u64,
        holds: usize,
        at: Instant,
    },
    Disarmed,
}

pub(crate) struct Watchdog {
    tick_tx: Sender<Tick>,
    next_tick: u64,
    /// Causes watchdog to shut down when dropped.
    _close_tx: Sender<()>,
}

impl Watchdog {
    pub(crate) fn new() -> Self {
        let (tick_tx, tick_rx) = unbounded();
        let (close_tx, close_rx) = unbounded();

        spawn_with_stack_size!("oak-watchdog", stdext::TINY_STACK_SIZE, move || poll(
            tick_rx, close_rx
        ));

        Self {
            tick_tx,
            _close_tx: close_tx,
            next_tick: 0,
        }
    }

    /// Arm the watchdog for one tick. Dropping the returned guard disarms it
    /// again, on every exit path out of `handle_event` including `?` early
    /// returns and panics.
    pub(crate) fn tick(&mut self, holds: usize) -> TickGuard {
        self.next_tick += 1;
        self.tick_tx
            .send(Tick::Armed {
                tick: self.next_tick,
                holds,
                at: Instant::now(),
            })
            .log_err();

        TickGuard {
            tick_tx: self.tick_tx.clone(),
        }
    }
}

/// Owns the arm for one main-loop tick.
pub(crate) struct TickGuard {
    tick_tx: Sender<Tick>,
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        self.tick_tx.send(Tick::Disarmed).log_err();
    }
}

/// Wait for the next arm, then watch it until it disarms, rearms, or
/// `close_rx` disconnects (the watchdog was dropped).
fn poll(tick_rx: Receiver<Tick>, close_rx: Receiver<()>) {
    'idle: loop {
        let msg = select! {
            recv(tick_rx) -> msg => msg,
            recv(close_rx) -> _ => return,
        };

        let Ok(Tick::Armed {
            mut tick,
            mut holds,
            mut at,
        }) = msg
        else {
            // A stray `Disarmed`: nothing is armed yet, keep waiting.
            continue 'idle;
        };

        // Report once `at` crosses `DEADLINE`, then every `DEADLINE` after that.
        loop {
            let msg = select! {
                recv(tick_rx) -> msg => Some(msg),
                recv(close_rx) -> _ => return,
                default(DEADLINE) => None,
            };

            match msg {
                Some(Ok(Tick::Disarmed)) => continue 'idle,
                Some(Err(_)) => return,
                None => report(tick, at.elapsed(), holds),
                // Two arms without a disarm between them shouldn't happen
                // (only one tick runs at a time), but if it does, start
                // watching the new one instead of reporting on the stale one.
                Some(Ok(Tick::Armed {
                    tick: new_tick,
                    holds: new_holds,
                    at: new_at,
                })) => {
                    log::warn!("Unexpected `Tick::Armed` before `Tick::Disarmed`");
                    tick = new_tick;
                    holds = new_holds;
                    at = new_at;
                },
            }
        }
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
    use crossbeam::channel::unbounded;
    use crossbeam::channel::Receiver;

    use super::Tick;
    use super::Watchdog;

    impl Watchdog {
        /// Build a watchdog without spawning the poller thread, exposing the
        /// tick channel so tests can assert on the messages `tick()` and
        /// `TickGuard::drop()` send.
        fn new_test() -> (Self, Receiver<Tick>) {
            let (tick_tx, tick_rx) = unbounded();
            let (close_tx, _close_rx) = unbounded();

            let watchdog = Self {
                tick_tx,
                _close_tx: close_tx,
                next_tick: 0,
            };
            (watchdog, tick_rx)
        }
    }

    #[test]
    fn test_tick_arms_and_disarms() {
        let (mut watchdog, tick_rx) = Watchdog::new_test();

        let guard = watchdog.tick(3);
        let Ok(Tick::Armed { holds, .. }) = tick_rx.try_recv() else {
            panic!("expected `Armed`");
        };
        assert_eq!(holds, 3);

        drop(guard);
        assert!(matches!(tick_rx.try_recv(), Ok(Tick::Disarmed)));
    }

    #[test]
    fn test_consecutive_ticks_increase() {
        let (mut watchdog, tick_rx) = Watchdog::new_test();

        let _first = watchdog.tick(0);
        let Ok(Tick::Armed {
            tick: first_tick, ..
        }) = tick_rx.try_recv()
        else {
            panic!("expected `Armed`");
        };

        let _second = watchdog.tick(0);
        let Ok(Tick::Armed {
            tick: second_tick, ..
        }) = tick_rx.try_recv()
        else {
            panic!("expected `Armed`");
        };

        assert!(second_tick > first_tick);
    }
}
