//
// deadlock.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::thread;
use std::time::Duration;

use parking_lot::deadlock;
use stdext::spawn;

/// Watch for `parking_lot::Mutex` deadlocks, particularly for the
/// `R_RUNTIME_LOCK` used by `r_lock!`. This allows us to determine
/// if and where we have called `r_lock!` from within another `r_lock!`.
pub fn watch() {
    spawn!("ark-deadlock-watcher", || { deadlock_thread() });
}

fn deadlock_thread() {
    loop {
        // Check for deadlocks every 20 seconds
        thread::sleep(Duration::from_secs(20));

        let deadlocks = deadlock::check_deadlock();
        if deadlocks.is_empty() {
            log::info!("No new deadlocks detected.");
            continue;
        }

        log::error!("{} deadlock(s) detected.", deadlocks.len());

        for (i, threads) in deadlocks.iter().enumerate() {
            log::error!("Deadlock #{}", i + 1);

            for thread in threads {
                log::error!(
                    "Thread Id: {:#?}\nBacktrace:\n{:#?}",
                    thread.thread_id(),
                    thread.backtrace()
                );
            }
        }
    }
}
