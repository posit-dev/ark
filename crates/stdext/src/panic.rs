//
// panic.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Thread-local panic recovery boundaries for process-wide panic hooks.
//!
//! Hooks use `recovers_panic()` to decide whether to allow unwinding instead of
//! aborting. Boundary assertions use `in_catch_boundary()` to check whether a
//! boundary is declared, even when its recovery policy disallows unwinding.
//! Both read the same thread-local state so hooks and assertions agree about
//! which boundary is active.

use std::any::Any;
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::panic::AssertUnwindSafe;
use std::task::Poll;

pub type PanicPayload = Box<dyn Any + Send + 'static>;

/// The message out of a panic payload, for logging. Takes `&dyn Any` so it
/// serves both a `catch_unwind()` payload and `PanicHookInfo::payload()`.
///
/// `panic!()` boxes its message as `&str` when it has no arguments to format
/// and as `String` when it does, so both need handling. Anything else comes
/// from `panic_any()`.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        msg.to_string()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        String::from("(unknown panic payload)")
    }
}

/// Which builds a boundary recovers panics in.
#[derive(Clone, Copy)]
pub enum Recovery {
    /// Recover in every build.
    Always,
    /// Recover in release builds only, so a bug surfaces as an abort during
    /// development instead of being converted into an ordinary error. For
    /// example, Ark uses it for panics in R callbacks, which would otherwise
    /// surface as R errors.
    ReleaseOnly,
}

thread_local! {
    static BOUNDARY: Cell<Option<Recovery>> = const { Cell::new(None) };
}

/// Whether a `catch_unwind()` boundary is waiting to recover this panic.
pub fn recovers_panic() -> bool {
    match BOUNDARY.get() {
        None => false,
        Some(Recovery::Always) => true,
        Some(Recovery::ReleaseOnly) => !cfg!(debug_assertions),
    }
}

/// Whether a `catch_unwind()` boundary is declared on this thread. Unlike
/// [`recovers_panic()`], this includes `ReleaseOnly` boundaries in debug builds,
/// which abort rather than recover.
pub fn in_catch_boundary() -> bool {
    BOUNDARY.get().is_some()
}

/// Assert that code which can panic by design runs inside a declared `catch_unwind()`
/// boundary, so its panics are recovered rather than aborting the process. For
/// example, Ark asserts this in Salsa database access, which panics on query
/// cycles, and in `try_r_task()`, which re-raises panics from its closure on the
/// caller's thread.
///
/// Unit tests can invoke handlers without their normal boundaries. This macro
/// evaluates `cfg!(test)` in the calling crate to exempt those tests.
/// The `testing` feature gives no exemption, since a crate's dev-dependency on
/// itself can enable it for integration tests and benchmarks too. Those callers
/// must enter a boundary, for example with `test_boundary()`.
#[macro_export]
macro_rules! assert_in_catch_boundary {
    () => {
        if !(cfg!(test) || $crate::panic::in_catch_boundary()) {
            $crate::debug_panic!("assertion failed: cfg!(test) || in_catch_boundary()");
        }
    };
}

/// Allow synchronous harness code to unwind to the test or benchmark runner
/// instead of aborting in a hook that checks [`recovers_panic()`]. This guard
/// does not catch panics. Dropping it restores the previous boundary state.
///
/// Tests also enter it to satisfy [`assert_in_catch_boundary!()`] in code that
/// production reaches only through a recovery boundary. Enter it in the function
/// that reaches the asserting code: a test helper guards its own access, and a
/// test body guards itself only where it reaches that code directly. Guards nest,
/// so calling a guarded helper under a body guard is fine. Unit tests of the crate
/// that calls the assertion are exempt, but can still enter it so they keep
/// passing if the asserting code moves to another crate.
///
/// Keep the guard in a local variable. Each guard restores the state from when
/// it was created, so guards must drop in reverse order of creation, which a
/// guard stored in a fixture can't ensure.
///
/// Never hold this guard across `.await`: its state is thread-local, so unrelated
/// tasks could inherit it and the guard could be dropped on a different thread.
/// Async tests enter a separate guard for each synchronous section.
#[cfg(any(test, feature = "testing"))]
#[must_use]
pub fn test_boundary() -> CatchBoundary {
    catch_boundary(Recovery::Always)
}

/// Install a panic hook that aborts on unrecovered panics.
///
/// When [`recovers_panic()`] is false, call the previous hook and abort the
/// process. This makes a panic that escapes a worker's recovery boundary fail
/// the test process instead of only ending the worker thread.
#[cfg(any(test, feature = "testing"))]
pub fn install_abort_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        if recovers_panic() {
            return;
        }
        default_hook(panic_info);
        std::process::abort();
    }));
}

/// Restore this thread's boundary state when the returned guard drops, without
/// entering a boundary.
///
/// A foreign unwind or `longjmp` skips the destructors of the Rust frames it
/// crosses. If it skips a [`CatchBoundary`] guard, the thread stays marked as
/// recovering panics, so a later panic on it would unwind instead of aborting.
/// Hold this guard in a frame outside the jump's landing point so it drops
/// normally afterwards. For example, Ark holds it in the caller of `r_sandbox()`,
/// where R errors longjump to.
#[must_use]
pub fn preserve_boundary() -> CatchBoundary {
    CatchBoundary {
        previous: BOUNDARY.get(),
        _not_send: PhantomData,
    }
}

/// Runs `f` inside a `catch_unwind()` boundary. `Err` carries the panic message, and
/// unwind safety is asserted on the caller's behalf.
pub fn catch_unwind<T>(recovery: Recovery, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind_payload(recovery, f).map_err(|payload| panic_message(payload.as_ref()))
}

/// [`catch_unwind()`] for a caller that hands the panic on to `resume_unwind()`
/// elsewhere, or that must classify the payload, and so needs the payload rather
/// than a message. For example, a typed payload such as `salsa::Cancelled` signals
/// cancellation rather than a bug.
pub fn catch_unwind_payload<T>(
    recovery: Recovery,
    f: impl FnOnce() -> T,
) -> std::thread::Result<T> {
    let _boundary = catch_boundary(recovery);
    std::panic::catch_unwind(AssertUnwindSafe(f))
}

/// Recover panics while polling a future. Enter the recovery boundary for each poll so
/// unrelated work on the polling thread cannot inherit it.
pub async fn catch_unwind_async<T>(
    recovery: Recovery,
    future: impl Future<Output = T>,
) -> Result<T, String> {
    catch_unwind_async_payload(recovery, future)
        .await
        .map_err(|payload| panic_message(payload.as_ref()))
}

/// [`catch_unwind_async()`] for a caller that must classify the payload, for
/// example to tell a typed payload such as `salsa::Cancelled` from a genuine panic.
pub async fn catch_unwind_async_payload<T>(
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

pub fn message(payload: &PanicPayload) -> String {
    panic_message(payload.as_ref())
}

/// Guard that preserves a `catch_unwind()` recovery boundary for the panic hook.
///
/// Restores the preceding flag in `Drop::drop()` so a nested boundary cannot disable an
/// outer boundary.
///
/// The guard is neither `Send` nor `Sync`. Its `Drop::drop()` restores the state
/// of whichever thread drops it, so a guard dropped on another thread would leave
/// its own thread's boundary in place:
///
/// ```compile_fail,E0277
/// fn assert_send<T: Send>() {}
/// assert_send::<stdext::panic::CatchBoundary>();
/// ```
///
/// ```compile_fail,E0277
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<stdext::panic::CatchBoundary>();
/// ```
pub struct CatchBoundary {
    previous: Option<Recovery>,
    // The raw pointer opts out of `Send` and `Sync`, see above.
    _not_send: PhantomData<*const ()>,
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
    CatchBoundary {
        previous,
        _not_send: PhantomData,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    #[test]
    fn test_panic_message_handles_both_payload_types() {
        let unformatted = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
        assert_eq!(panic_message(unformatted.as_ref()), "plain");

        let formatted = std::panic::catch_unwind(|| panic!("formatted {}", 1)).unwrap_err();
        assert_eq!(panic_message(formatted.as_ref()), "formatted 1");

        let other = std::panic::catch_unwind(|| std::panic::panic_any(42u8)).unwrap_err();
        assert_eq!(panic_message(other.as_ref()), "(unknown panic payload)");
    }

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
    fn test_test_boundary_restores_scope() {
        assert!(!in_catch_boundary());
        {
            let _boundary = test_boundary();
            assert!(recovers_panic());
        }
        assert!(!in_catch_boundary());
    }

    #[test]
    fn test_test_boundary_restores_outer_boundary() {
        let _outer = catch_boundary(Recovery::ReleaseOnly);
        {
            let _boundary = test_boundary();
            assert!(recovers_panic());
        }
        assert!(matches!(BOUNDARY.get(), Some(Recovery::ReleaseOnly)));
    }

    #[test]
    fn test_test_boundary_propagates_panic_and_restores_outer_boundary() {
        let _outer = catch_boundary(Recovery::ReleaseOnly);
        let result = std::panic::catch_unwind(|| {
            let _boundary = test_boundary();
            assert!(recovers_panic());
            panic!("harness panic");
        });
        let Err(payload) = result else {
            panic!("Expected a panic payload");
        };
        assert_eq!(message(&payload), "harness panic");
        assert!(matches!(BOUNDARY.get(), Some(Recovery::ReleaseOnly)));
    }

    #[test]
    fn test_preserve_boundary_restores_state_after_skipped_guard() {
        let _outer = catch_boundary(Recovery::ReleaseOnly);
        {
            let _preserved = preserve_boundary();

            // Stands in for an R longjump skipping the guard's destructor
            std::mem::forget(catch_boundary(Recovery::Always));
            assert!(recovers_panic());
        }
        assert!(matches!(BOUNDARY.get(), Some(Recovery::ReleaseOnly)));
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

    /// Selects the scenario `test_abort_hook_child()` runs when this test binary
    /// is re-run as a child process. Unset in a normal test run.
    const ABORT_CHILD_ENV: &str = "STDEXT_TEST_ABORT_HOOK_CHILD";
    const ABORT_CHILD_RECOVERED: &str = "abort hook child recovered";

    #[test]
    fn test_abort_hook_child() {
        let Ok(mode) = std::env::var(ABORT_CHILD_ENV) else {
            return;
        };
        install_abort_hook();

        match mode.as_str() {
            "unrecovered" => panic!("unrecovered panic"),
            "recovered" => {
                let result = catch_unwind(Recovery::Always, || panic!("recovered panic"));
                assert_eq!(result, Err(String::from("recovered panic")));
                println!("{ABORT_CHILD_RECOVERED}");
            },
            _ => panic!("Unknown mode: {mode}"),
        }
    }

    #[test]
    fn test_install_abort_hook_aborts_unrecovered_panic() {
        let output = run_abort_hook_child("unrecovered");

        // `SIGABRT`, without depending on `libc` for one constant
        #[cfg(unix)]
        assert_eq!(output.status.signal(), Some(6));
        #[cfg(not(unix))]
        assert!(!output.status.success());
    }

    #[test]
    fn test_install_abort_hook_lets_boundary_recover() {
        let output = run_abort_hook_child("recovered");
        assert!(output.status.success());

        // Rules out a child that ran no test because the filter matched nothing
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(ABORT_CHILD_RECOVERED));
    }

    /// Re-run this test binary with only `test_abort_hook_child()`, so its panic
    /// hook and any abort stay in the child process.
    fn run_abort_hook_child(mode: &str) -> std::process::Output {
        let exe = std::env::current_exe().unwrap();
        std::process::Command::new(exe)
            .args([
                "panic::tests::test_abort_hook_child",
                "--exact",
                "--nocapture",
            ])
            .env(ABORT_CHILD_ENV, mode)
            .output()
            .unwrap()
    }

    /// Stands in for a typed unwind payload such as `salsa::Cancelled`.
    struct Cancelled;

    #[tokio::test]
    async fn test_catch_unwind_async_payload_preserves_typed_payload() {
        let result = catch_unwind_async_payload(Recovery::Always, async {
            std::panic::resume_unwind(Box::new(Cancelled))
        })
        .await;

        let Err(payload) = result else {
            panic!("Expected a cancellation payload");
        };
        assert!(payload.is::<Cancelled>());
    }
}
