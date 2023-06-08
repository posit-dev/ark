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
// This uses `signal()` instead of `sigaction()` for Windows support
// (SIGSEGV is one of the rare supported signals)
pub fn register_trap_handlers() {
    unsafe {
        libc::signal(libc::SIGBUS, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGSEGV, backtrace_handler as libc::sighandler_t);
    }
}

extern "C" fn backtrace_handler(signum: libc::c_int) {
    // Prevent infloop into the handler
    unsafe {
        libc::signal(signum, libc::SIG_DFL);
    }

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
