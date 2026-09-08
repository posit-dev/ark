//
// panic.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Logs panics before `catch_unwind()` handles them. `catch_unwind()` prevents a
//! recovered panic from aborting the process. Kept outside `main.rs` so tests can
//! install it.

use std::cell::Cell;

use stdext::panic_message;

/// Install the global panic hook.
///
/// This causes panics on background threads to propagate on the main
/// thread. If we don't propagate a background thread panic, the program
/// keeps running in an unstable state as all communications with this
/// thread will error out or panic.
/// https://stackoverflow.com/questions/35988775/how-can-i-cause-a-panic-on-a-thread-to-immediately-end-the-main-thread
///
/// Log panics and abort the process unless a recovery boundary or Tokio handles them.
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

        // A current Tokio handle may be unrelated to the LSP, but Tokio captures task
        // panics for its caller to handle.
        if tokio::runtime::Handle::try_current().is_ok() {
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

/// Which builds a boundary recovers panics in.
#[derive(Clone, Copy)]
pub(crate) enum Recovery {
    /// Recover in every build.
    Always,
    /// Recover in release builds only. A panic in an R callback aborts during
    /// development instead of surfacing as an R error.
    ReleaseOnly,
}

thread_local! {
    static BOUNDARY: Cell<Option<Recovery>> = const { Cell::new(None) };
}

/// Whether a `catch_unwind()` boundary is waiting to recover this panic.
fn recovers_panic() -> bool {
    match BOUNDARY.get() {
        None => false,
        Some(Recovery::Always) => true,
        Some(Recovery::ReleaseOnly) => !cfg!(debug_assertions),
    }
}

/// Runs `f` inside a `catch_unwind()` boundary. `Err` carries the panic message, and
/// unwind safety is asserted on the caller's behalf.
pub(crate) fn catch_unwind<T>(recovery: Recovery, f: impl FnOnce() -> T) -> Result<T, String> {
    let _boundary = catch_boundary(recovery);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|payload| panic_message(payload.as_ref()))
}

/// Guard that preserves a `catch_unwind()` recovery boundary for the panic hook.
///
/// Restores the preceding flag in `Drop::drop()` so a nested boundary cannot disable an
/// outer boundary.
struct CatchBoundary {
    previous: Option<Recovery>,
}

impl Drop for CatchBoundary {
    fn drop(&mut self) {
        BOUNDARY.set(self.previous);
    }
}

/// Prevent the hook from aborting a panic handled by `catch_unwind()`.
fn catch_boundary(recovery: Recovery) -> CatchBoundary {
    let previous = BOUNDARY.get();
    BOUNDARY.set(Some(recovery));
    CatchBoundary { previous }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recovers_panic_false_without_boundary() {
        assert!(!recovers_panic());
    }

    #[test]
    fn test_recovers_panic_true_inside_always_boundary() {
        let _boundary = catch_boundary(Recovery::Always);
        assert!(recovers_panic());
    }

    #[test]
    fn test_nested_always_boundary_recovers_inside_release_only() {
        let _outer = catch_boundary(Recovery::ReleaseOnly);
        {
            let _inner = catch_boundary(Recovery::Always);
            assert!(recovers_panic());
        }
        assert_eq!(recovers_panic(), !cfg!(debug_assertions));
    }

    #[test]
    fn test_dropping_boundary_leaves_none() {
        {
            let _boundary = catch_boundary(Recovery::Always);
            assert!(recovers_panic());
        }
        assert!(!recovers_panic());
    }

    #[test]
    fn test_catch_unwind_passes_through_ok() {
        let result = catch_unwind(Recovery::Always, || 1 + 1);
        assert_eq!(result, Ok(2));
    }

    #[test]
    fn test_catch_unwind_converts_panic_to_err() {
        let result = catch_unwind(Recovery::Always, || panic!("oh no"));
        assert_eq!(result, Err(String::from("oh no")));
    }
}
