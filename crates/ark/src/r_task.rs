//
// r_task.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

// Re-export infrastructure from the `r_task` crate so the rest of ark can
// use `crate::r_task::` without needing to reference `::r_task::` directly.
// We use `::r_task::` (absolute path) because this module shares the name.
pub use ::r_task::on_main_thread;
pub use ::r_task::r_task;
pub use ::r_task::set_initialized;
pub use ::r_task::set_main_thread;
pub use ::r_task::set_test_init_hook;
pub use ::r_task::take_receivers;
pub use ::r_task::BoxFuture;
pub use ::r_task::QueuedTask as QueuedRTask;
pub use ::r_task::TaskStartInfo as RTaskStartInfo;
pub use ::r_task::TaskStatus as RTaskStatus;
pub use ::r_task::TaskWaker as RTaskWaker;
use libr::SEXP;

use crate::console::Console;
use crate::console::ConsoleOutputCapture;

/// An async task to be run on the R thread.
///
/// Construct via `RTask::interrupt`, `RTask::idle`, or `RTask::idle_any_prompt`
/// when spawning from the R thread. Use the `Send` variants
/// (`RTask::send_interrupt`, etc.) when spawning from other threads.
///
/// For idle modes, console output is automatically captured during the task's
/// execution via a `ConsoleOutputCapture` passed to the closure.
pub(crate) enum RTask {
    /// Run at the next interrupt check. Must be spawned from the R thread.
    Interrupt(BoxFuture<'static, ()>),
    /// Run when R is at a top-level idle prompt. Must be spawned from the R thread.
    Idle(BoxFuture<'static, ()>),
    /// Run when R is at any idle prompt (top-level or browser). Must be spawned
    /// from the R thread.
    IdleAnyPrompt(BoxFuture<'static, ()>),
    /// Like `Interrupt`, but can be spawned from any thread. The constructor
    /// enforces `Send` on the closure.
    SendInterrupt(BoxFuture<'static, ()>),
    /// Like `Idle`, but can be spawned from any thread. The constructor
    /// enforces `Send` on the closure.
    SendIdle(BoxFuture<'static, ()>),
    /// Like `IdleAnyPrompt`, but can be spawned from any thread. The constructor
    /// enforces `Send` on the closure.
    SendIdleAnyPrompt(BoxFuture<'static, ()>),
}

impl RTask {
    pub(crate) fn interrupt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::Interrupt(Box::pin(fun()))
    }

    pub(crate) fn idle<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::Idle(pin_with_capture(fun))
    }

    #[allow(unused)]
    pub(crate) fn idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::IdleAnyPrompt(pin_with_capture(fun))
    }

    pub(crate) fn send_interrupt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendInterrupt(Box::pin(fun()))
    }

    pub(crate) fn send_idle<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdle(pin_with_capture(fun))
    }

    pub(crate) fn send_idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdleAnyPrompt(pin_with_capture(fun))
    }
}

fn pin_with_capture<F, Fut>(fun: F) -> BoxFuture<'static, ()>
where
    F: FnOnce(ConsoleOutputCapture) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
{
    Box::pin(async move {
        let capture = start_capture();
        fun(capture).await
    })
}

/// Spawn an async task on the R thread.
///
/// Converts the ark-local `RTask` to `::r_task::RTask` and delegates to
/// the crate-level `spawn()`.
pub(crate) fn spawn(task: RTask) {
    let task = match task {
        RTask::Interrupt(fut) => ::r_task::RTask::Interrupt(fut),
        RTask::Idle(fut) => ::r_task::RTask::Idle(fut),
        RTask::IdleAnyPrompt(fut) => ::r_task::RTask::IdleAnyPrompt(fut),
        RTask::SendInterrupt(fut) => ::r_task::RTask::SendInterrupt(fut),
        RTask::SendIdle(fut) => ::r_task::RTask::SendIdle(fut),
        RTask::SendIdleAnyPrompt(fut) => ::r_task::RTask::SendIdleAnyPrompt(fut),
    };
    ::r_task::spawn(task);
}

fn start_capture() -> ConsoleOutputCapture {
    if Console::is_initialized() {
        Console::get_mut().start_capture()
    } else {
        // Unit tests run without a Console. The dummy capture is
        // inert and doesn't interact with Console state.
        debug_assert!(stdext::IS_TESTING);
        ConsoleOutputCapture::dummy()
    }
}

// Test-only R-callable functions for spawning a pending idle task.
//
// This allows integration tests to exercise the `captured_output`
// save/restore mechanism in `poll_task`. The flow is:
//
// 1. Test calls `.Call("ps_test_spawn_pending_task")` from R.
//    This spawns an async idle task that creates a `ConsoleOutputCapture`
//    and then awaits a oneshot channel (staying Pending).
//
// 2. On the next event-loop iteration the task is polled, `captured_output`
//    is set, and the future yields. `poll_task` should save the capture
//    into `pending_futures` and clear `captured_output`.
//
// 3. The test busy-loops with `getOption("ark.test.task_polled")` until it
//    sees `TRUE`, confirming the task has been polled.
//
// 4. The test sends another execute request (e.g. `cat("hello\n")`).
//    Because `captured_output` has been cleared, the output reaches IOPub.
//
// 5. Test calls `.Call("ps_test_complete_pending_task")` to unblock the
//    oneshot, letting the idle task finish and drop its capture cleanly.

#[cfg(debug_assertions)]
static TEST_PENDING_TASK_TX: Mutex<Option<futures::channel::oneshot::Sender<()>>> =
    Mutex::new(None);

#[cfg(debug_assertions)]
#[harp::register]
unsafe extern "C-unwind" fn ps_test_spawn_pending_task() -> anyhow::Result<SEXP> {
    stdext::assert_testing();

    // Reset the flag before spawning
    harp::parse_eval_base("options(ark.test.task_polled = FALSE)")?;

    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    *TEST_PENDING_TASK_TX.lock().unwrap() = Some(tx);

    spawn(RTask::idle(async move |_| {
        // Signal that we've been polled (capture is now active)
        harp::parse_eval_base("options(ark.test.task_polled = TRUE)").ok();

        // Stay pending until the test signals completion
        let _ = rx.await;

        // Clean up
        harp::parse_eval_base("options(ark.test.task_polled = NULL)").ok();
    }));

    Ok(libr::R_NilValue)
}

/// Signal the pending idle task to complete. The oneshot sender is
/// consumed, the task's future resolves, and its `ConsoleOutputCapture`
/// is dropped (restoring the previous capture state).
#[cfg(debug_assertions)]
#[harp::register]
unsafe extern "C-unwind" fn ps_test_complete_pending_task() -> anyhow::Result<SEXP> {
    stdext::assert_testing();

    if let Some(tx) = TEST_PENDING_TASK_TX.lock().unwrap().take() {
        let _ = tx.send(());
    }

    Ok(libr::R_NilValue)
}

/// Spawn `n` idle tasks that each sleep for `sleep_ms` milliseconds.
///
/// Used by integration tests to create guaranteed contention between idle
/// tasks and kernel requests in the event loop. With the priority fix,
/// kernel requests are always serviced before these sleeping tasks.
#[cfg(debug_assertions)]
#[harp::register]
unsafe extern "C-unwind" fn ps_test_spawn_sleeping_idle_tasks(
    n: SEXP,
    sleep_ms: SEXP,
) -> anyhow::Result<SEXP> {
    stdext::assert_testing();

    let n: i32 = harp::RObject::view(n).try_into()?;
    let sleep_ms: i32 = harp::RObject::view(sleep_ms).try_into()?;
    let sleep_duration = Duration::from_millis(sleep_ms as u64);

    for _ in 0..n {
        spawn(RTask::idle(async move |_capture| {
            std::thread::sleep(sleep_duration);
        }));
    }

    Ok(libr::R_NilValue)
}
