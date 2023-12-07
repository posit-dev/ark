/*
 * signals.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use libR_shim::R_interrupts_pending;
use nix::sys::signal::*;

/// Reset the signal block.
///
/// This appears to be necessary on macOS; 'sigprocmask()' specifically
/// blocks the signals in _all_ threads associated with the process, even
/// when called from a spawned child thread. See:
///
/// https://github.com/opensource-apple/xnu/blob/0a798f6738bc1db01281fc08ae024145e84df927/bsd/kern/kern_sig.c#L1238-L1285
/// https://github.com/opensource-apple/xnu/blob/0a798f6738bc1db01281fc08ae024145e84df927/bsd/kern/kern_sig.c#L796-L839
///
/// and note that 'sigprocmask()' uses 'block_procsigmask()' to apply the
/// requested block to all threads in the process:
///
/// https://github.com/opensource-apple/xnu/blob/0a798f6738bc1db01281fc08ae024145e84df927/bsd/kern/kern_sig.c#L571-L599
///
/// We may need to re-visit this on Linux later on, since 'sigprocmask()' and
/// 'pthread_sigmask()' may only target the executing thread there.
///
/// The behavior of 'sigprocmask()' is unspecified after all, so we're really
/// just relying on what the implementation happens to do.
pub fn initialize_signal_handlers() {
    let mut sigset = SigSet::empty();
    sigset.add(SIGINT);
    sigprocmask(SigmaskHow::SIG_BLOCK, Some(&sigset), None).unwrap();

    // Unblock signals on this thread.
    pthread_sigmask(SigmaskHow::SIG_UNBLOCK, Some(&sigset), None).unwrap();

    // Install an interrupt handler.
    unsafe {
        signal(SIGINT, SigHandler::Handler(handle_interrupt)).unwrap();
    }
}

/// Block signals in this thread (and any child threads).
///
/// Any threads that would like to handle signals should explicitly
/// unblock the signals they want to handle. This allows us to ensure
/// that interrupts are consistently handled on the same thread.
pub fn initialize_signal_block() {
    let mut sigset = SigSet::empty();
    sigset.add(SIGINT);
    sigprocmask(SigmaskHow::SIG_BLOCK, Some(&sigset), None).unwrap();
}

pub fn interrupts_pending() -> bool {
    unsafe { R_interrupts_pending == 1 }
}

pub fn set_interrupts_pending(pending: bool) {
    if pending {
        unsafe { R_interrupts_pending = 1 };
    } else {
        unsafe { R_interrupts_pending = 0 };
    }
}

pub extern "C" fn handle_interrupt(_signal: libc::c_int) {
    set_interrupts_pending(true);
}
