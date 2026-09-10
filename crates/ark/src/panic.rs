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
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::task::Poll;

use stdext::panic_message;

pub(crate) type PanicPayload = Box<dyn std::any::Any + Send + 'static>;

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
pub(crate) fn recovers_panic() -> bool {
    match BOUNDARY.get() {
        None => false,
        Some(Recovery::Always) => true,
        Some(Recovery::ReleaseOnly) => !cfg!(debug_assertions),
    }
}

/// Whether a `catch_unwind()` boundary is declared on this thread. Unlike
/// [`recovers_panic()`], this includes `ReleaseOnly` boundaries in debug builds,
/// which abort rather than recover.
fn in_catch_boundary() -> bool {
    BOUNDARY.get().is_some()
}

/// Assert that Salsa database access runs inside a declared `catch_unwind()` boundary.
/// Salsa queries can panic on cycles.
///
/// Tests invoke LSP handlers without their normal boundaries, so `cfg!(test)` exempts
/// them.
pub(crate) fn assert_in_catch_boundary() {
    debug_assert!(cfg!(test) || in_catch_boundary());
}

/// Runs `f` inside a `catch_unwind()` boundary. `Err` carries the panic message, and
/// unwind safety is asserted on the caller's behalf.
pub(crate) fn catch_unwind<T>(recovery: Recovery, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind_payload(recovery, f).map_err(|payload| panic_message(payload.as_ref()))
}

/// [`catch_unwind()`] for a caller that hands the panic on to `resume_unwind()`
/// elsewhere, or that must classify the payload (e.g. distinguish `salsa::Cancelled`
/// from a genuine panic), and so needs the payload rather than a message.
#[expect(clippy::disallowed_methods)]
pub(crate) fn catch_unwind_payload<T>(
    recovery: Recovery,
    f: impl FnOnce() -> T,
) -> std::thread::Result<T> {
    let _boundary = catch_boundary(recovery);
    std::panic::catch_unwind(AssertUnwindSafe(f))
}

/// Recover panics while polling a future. Enter the recovery boundary for each poll so
/// unrelated work on the polling thread cannot inherit it.
pub(crate) async fn catch_unwind_async<T>(
    recovery: Recovery,
    future: impl Future<Output = T>,
) -> Result<T, String> {
    catch_unwind_async_payload(recovery, future)
        .await
        .map_err(|payload| panic_message(payload.as_ref()))
}

/// [`catch_unwind_async()`] for a caller that must classify the payload, such as the
/// LSP event loop distinguishing `salsa::Cancelled` from a genuine panic.
#[expect(clippy::disallowed_methods)]
pub(crate) async fn catch_unwind_async_payload<T>(
    recovery: Recovery,
    future: impl Future<Output = T>,
) -> Result<T, PanicPayload> {
    let mut future = Box::pin(future);

    std::future::poll_fn(move |cx| {
        let _boundary = catch_boundary(recovery);

        match std::panic::catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    })
    .await
}

pub(crate) fn message(payload: &PanicPayload) -> String {
    panic_message(payload.as_ref())
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
    fn test_in_catch_boundary_ignores_recovery_kind() {
        assert!(!in_catch_boundary());

        let _boundary = catch_boundary(Recovery::ReleaseOnly);
        assert!(in_catch_boundary());
    }

    #[test]
    fn test_catch_unwind_passes_through_ok() {
        let result = catch_unwind(Recovery::Always, || 1 + 1);
        assert_eq!(result, Ok(2));
    }

    #[test]
    fn test_catch_unwind_converts_panic_to_message() {
        let result = catch_unwind(Recovery::Always, || panic!("oh no"));
        assert_eq!(result, Err(String::from("oh no")));
    }

    #[test]
    fn test_catch_unwind_payload_preserves_panic_payload() {
        let result = catch_unwind_payload(Recovery::Always, || panic!("oh no"));
        let Err(payload) = result else {
            panic!("Expected a panic payload");
        };
        assert_eq!(message(&payload), "oh no");
    }

    #[tokio::test]
    async fn test_catch_unwind_async_passes_through_ok() {
        let result = catch_unwind_async(Recovery::Always, async { 1 + 1 }).await;
        assert_eq!(result, Ok(2));
    }

    #[tokio::test]
    async fn test_catch_unwind_async_converts_panic_to_message() {
        let result = catch_unwind_async(Recovery::Always, async { panic!("oh no") }).await;
        assert_eq!(result, Err(String::from("oh no")));
    }

    #[tokio::test]
    async fn test_catch_unwind_async_payload_preserves_panic_payload() {
        let result = catch_unwind_async_payload(Recovery::Always, async { panic!("oh no") }).await;
        let Err(payload) = result else {
            panic!("Expected a panic payload");
        };
        assert_eq!(message(&payload), "oh no");
    }

    #[tokio::test]
    async fn test_catch_unwind_async_payload_preserves_salsa_cancellation() {
        let result = catch_unwind_async_payload(Recovery::Always, async {
            std::panic::resume_unwind(Box::new(salsa::Cancelled::PendingWrite))
        })
        .await;

        let Err(payload) = result else {
            panic!("Expected a cancellation payload");
        };
        assert!(payload.is::<salsa::Cancelled>());
    }
}
