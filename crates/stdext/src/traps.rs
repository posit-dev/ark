//
// traps.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Call this after initialising the `log` package. Instruments SIGBUS,
// SIGSEGV, and SIGILL to generate a backtrace with `info` verbosity
// (lowest level so it's always reported).
//
// This uses `signal()` instead of `sigaction()` for Windows support
// (SIGSEGV is one of the rare supported signals)
//
// Note that Rust also has a SIGSEGV handler to catch stack overflows. In
// this case it displays an informative message and aborts the program (no
// segfaults in Rust!). Ideally we'd save the Rust handler and notify
// it. However the only safe way to notify an old handler on Unixes is to
// use `sigaction()` so that we get the information needed to determine the
// type of handler (old or new school). So we'd need to make a different
// implementation for Windows (which only supports old style) and for Unix,
// and this doesn't seem worth it.
pub fn register_trap_handlers() {
    unsafe {
        libc::signal(libc::SIGBUS, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGSEGV, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGILL, backtrace_handler as libc::sighandler_t);
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
