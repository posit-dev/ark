//
// traps.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Call this after initialising the `log` package. Instruments SIGBUS and
// SIGSEGV to generate a backtrace with `info` verbosity (lowest level so
// it's always reported).
//
// This could be supported on windows too (SIGSEGV is one of the rare
// supported signals) but with `libc::signal()` instead of
// `libc::sigaction()`
#[cfg(not(target_os = "windows"))]
pub fn register_trap_handlers() {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        action.sa_sigaction = backtrace_handler as libc::sighandler_t;

        libc::sigaction(libc::SIGBUS, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGSEGV, &action, std::ptr::null_mut());
    }
}

#[cfg(not(target_os = "windows"))]
pub fn reset_traps_handler() {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;

        libc::sigaction(libc::SIGBUS, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGSEGV, &action, std::ptr::null_mut());
    }
}

#[cfg(not(target_os = "windows"))]
extern "C" fn backtrace_handler(
    signum: libc::c_int,
    _info: *mut libc::siginfo_t,
    _data: *mut libc::c_void,
) {
    // Prevent infloop into the handler
    reset_traps_handler();

    let mut header = format!("\n>>> Backtrace for signal {}", signum);

    if let Some(name) = std::thread::current().name() {
        header = format!("{}\n>>> In thread {}", header, name);
    }

    // Unlike asynchronous signals, SIGSEGV and SIGBUS are synchronous and
    // always delivered to the thread that caused it, so we can just
    // capture the current thread's backtrace
    let bt = backtrace::Backtrace::new();
    log::info!("{}\n{:?}", header, bt);
}
