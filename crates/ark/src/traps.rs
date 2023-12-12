//
// traps.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Call this after initialising the `log` package. Instruments
// SIGSEGV, SIGILL, and SIGBUS (on Unix) to generate a backtrace with `info`
// verbosity (lowest level so it's always reported).
//
// This uses `signal()` instead of `sigaction()` for Windows support
// (SIGSEGV is one of the rare supported signals).
//
// Note that Rust also has a SIGSEGV handler to catch stack overflows. In
// this case it displays an informative message and aborts the program (no
// segfaults in Rust!). Ideally we'd save the Rust handler and notify
// it. However the only safe way to notify an old handler on Unixes is to
// use `sigaction()` so that we get the information needed to determine the
// type of handler (old or new school). We have different implementations for
// Unix vs Windows (which only supports old style) already, so we could go back
// and set this up now.
pub use crate::sys::traps::register_trap_handlers;

pub extern "C" fn backtrace_handler(signum: libc::c_int) {
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
