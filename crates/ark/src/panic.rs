//
// panic.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Logs panics before `catch_unwind()` handles them. `catch_unwind()` prevents a
//! recovered panic from aborting the process. Kept outside `main.rs` so tests can
//! install it.
//!
//! Also re-exports the recovery boundary from `stdext::panic`, so Ark code reaches
//! the boundary, the hook and `spawn()` through this one module.

use std::cell::Cell;
use std::future::Future;

pub(crate) use stdext::panic::*;
use stdext::panic_message;
use tokio::task::JoinHandle;

/// Install the global panic hook.
///
/// This causes panics on background threads to propagate on the main
/// thread. If we don't propagate a background thread panic, the program
/// keeps running in an unstable state as all communications with this
/// thread will error out or panic.
/// https://stackoverflow.com/questions/35988775/how-can-i-cause-a-panic-on-a-thread-to-immediately-end-the-main-thread
pub fn install() {
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let info = panic_info.payload();

        let loc = if let Some(location) = panic_info.location() {
            format!("In file '{}' at line {}:", location.file(), location.line(),)
        } else {
            String::from("No location information:")
        };

        let msg = panic_message(info);

        // Top-level-exec and try-catch errors already contain a backtrace
        // for the R thread so don't repeat it if we see one. Only perform
        // this check on the R thread because we do want other threads'
        // backtraces if the panic occurred elsewhere.
        let trace = if on_r_thread() && msg.contains("\n{R_BACKTRACE_HEADER}\n") {
            String::new()
        } else {
            format!("Backtrace:\n{}", std::backtrace::Backtrace::force_capture())
        };

        log::error!("Panic! {loc} {msg}\n{trace}");

        // A boundary has a `catch_unwind()` waiting for this panic. The
        // backtrace is already logged above.
        if recovers_panic() {
            // Return and let the panic continue unwinding to the catch site
            return;
        }

        // Leave time for the log sink to write the flushed panic before aborting.
        log::logger().flush();
        std::thread::sleep(std::time::Duration::from_millis(250));

        old_hook(panic_info);
        std::process::abort();
    }));
}

thread_local! {
    static ON_R_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Mark the calling thread as the main R thread, so the panic hook knows
/// whether an R backtrace is already present in the panic message.
pub fn mark_r_thread() {
    ON_R_THREAD.set(true);
}

fn on_r_thread() -> bool {
    ON_R_THREAD.get()
}

/// Catch and log task panics instead of letting the panic hook abort the process.
///
/// [`catch_unwind_async()`] declares a boundary only during each poll of its future.
/// Other tasks on the same worker thread cannot inherit that boundary, so a task
/// spawned with `tokio::spawn()` alone would abort on panic rather than fail its
/// `JoinHandle` as it would without Ark's panic hook.
#[expect(clippy::disallowed_methods)]
pub(crate) fn spawn(
    recovery: Recovery,
    future: impl Future<Output = ()> + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(msg) = catch_unwind_async(recovery, future).await {
            log::error!("Panic in spawned task: {msg}");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_recovers_panic_instead_of_propagating() {
        let handle = spawn(Recovery::Always, async { panic!("oh no") });
        assert!(handle.await.is_ok());
    }
}
