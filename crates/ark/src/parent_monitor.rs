//
// parent_monitor.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

//! Parent process monitoring for graceful shutdown on Linux.
//!
//! This module implements Linux-specific functionality to monitor the parent process
//! and trigger graceful shutdown of the kernel when the parent exits.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crossbeam::channel::Sender;
use stdext::spawn;

use crate::request::RRequest;

/// Flag to track if parent monitoring is active
static PARENT_MONITORING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Start monitoring the parent process for exit.
///
/// On Linux, this uses `prctl(PR_SET_PDEATHSIG)` to set up a signal that will be
/// delivered when the parent process exits. When this signal is received, a graceful
/// shutdown request is sent to the R execution thread.
///
/// # Arguments
/// * `r_request_tx` - Channel to send shutdown requests to the R execution thread
///
pub fn start_parent_monitoring(r_request_tx: Sender<RRequest>) -> anyhow::Result<()> {
    // Check if already active to avoid multiple monitors
    if PARENT_MONITORING_ACTIVE.swap(true, Ordering::SeqCst) {
        log::warn!("Parent process monitoring is already active");
        return Ok(());
    }

    log::info!("Starting parent process monitoring");

    // Set up SIGUSR1 to be delivered when parent dies
    const PR_SET_PDEATHSIG: libc::c_int = 1;
    const SIGUSR1: libc::c_int = 10;

    let result = unsafe { libc::prctl(PR_SET_PDEATHSIG, SIGUSR1, 0, 0, 0) };
    if result != 0 {
        PARENT_MONITORING_ACTIVE.store(false, Ordering::SeqCst);
        let errno = unsafe { *libc::__errno_location() };
        return Err(anyhow::anyhow!(
            "Failed to set parent death signal: errno {errno}"
        ));
    }

    // Spawn a thread to monitor for the signal and handle shutdown
    let r_request_tx_clone = r_request_tx.clone();
    spawn!("parent-monitor", move || {
        monitor_parent_death_signal(r_request_tx_clone);
    });

    log::info!("Parent process monitoring started successfully");
    Ok(())
}

fn monitor_parent_death_signal(r_request_tx: Sender<RRequest>) {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use nix::sys::signal::SigHandler;
    use nix::sys::signal::Signal;
    use nix::sys::signal::{self};

    // Use a static flag to signal when SIGUSR1 is received
    static SIGUSR1_RECEIVED: AtomicBool = AtomicBool::new(false);

    // Signal handler that just sets a flag
    extern "C" fn sigusr1_handler(_: libc::c_int) {
        SIGUSR1_RECEIVED.store(true, Ordering::SeqCst);
    }

    // Install signal handler for SIGUSR1
    unsafe {
        if let Err(err) = signal::signal(Signal::SIGUSR1, SigHandler::Handler(sigusr1_handler)) {
            log::error!("Failed to install SIGUSR1 handler: {err}");
            return;
        }
    }

    log::trace!("Parent death signal monitoring thread started");

    // Poll for the signal flag
    loop {
        if SIGUSR1_RECEIVED.load(Ordering::SeqCst) {
            log::info!("Parent process has exited, initiating graceful shutdown");

            // Send shutdown request to R execution thread (false = final shutdown, not restart)
            if let Err(err) = r_request_tx.send(RRequest::Shutdown(false)) {
                log::error!("Failed to send shutdown request, exiting: {err}");
                // If we can't send the shutdown request, force exit as fallback
                std::process::exit(1);
            }
            break;
        }

        // Sleep for 5s to avoid busy-waiting; the goal is to ensure Ark doesn't
        // hang around forever after the parent exits, no need for
        // high-frequency checks
        thread::sleep(Duration::from_millis(5000));
    }

    log::trace!("Parent death signal monitoring thread exiting");
}
