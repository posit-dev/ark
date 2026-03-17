//
// r_task.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use libr::SEXP;
use uuid::Uuid;

use crate::console::Console;
use crate::console::ConsoleOutputCapture;
use crate::fixtures::r_test_init;

/// Task channels for interrupt-time tasks
static INTERRUPT_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

/// Task channels for idle-time tasks
static IDLE_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

/// Task channels for idle tasks that run at any idle prompt (top-level or browser)
static IDLE_ANY_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

// Compared to `futures::BoxFuture`, this doesn't require the future to be Send.
// We don't need this bound since the executor runs on only on the R thread
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

type SharedOption<T> = Arc<Mutex<Option<T>>>;

/// Manages task channels for sending tasks to `CONSOLE`.
struct TaskChannels {
    tx: Sender<QueuedRTask>,
    rx: Mutex<Option<Receiver<QueuedRTask>>>,
}

impl TaskChannels {
    fn new() -> Self {
        let (tx, rx) = unbounded::<QueuedRTask>();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    fn tx(&self) -> Sender<QueuedRTask> {
        self.tx.clone()
    }

    fn take_rx(&self) -> Receiver<QueuedRTask> {
        let mut rx = self.rx.lock().unwrap();
        rx.take().expect("`take_rx()` can only be called once")
    }
}

/// Returns receivers for interrupt, idle, and debug-idle tasks.
/// Initializes the task channels if they haven't been initialized yet.
/// Can only be called once (intended for `Console` during init).
pub(crate) fn take_receivers() -> (
    Receiver<QueuedRTask>,
    Receiver<QueuedRTask>,
    Receiver<QueuedRTask>,
) {
    (
        INTERRUPT_TASKS.take_rx(),
        IDLE_TASKS.take_rx(),
        IDLE_ANY_TASKS.take_rx(),
    )
}

pub enum QueuedRTask {
    Sync(RTaskSync),
    Async(RTaskAsync),
    Parked(Arc<RTaskWaker>),
}

pub struct RTaskSync {
    pub fun: Box<dyn FnOnce() + Send + 'static>,
    pub status_tx: Option<Sender<RTaskStatus>>,
    pub start_info: RTaskStartInfo,
}

pub struct RTaskAsync {
    pub fut: BoxFuture<'static, ()>,
    pub tasks_tx: Sender<QueuedRTask>,
    pub start_info: RTaskStartInfo,
}

#[derive(Clone)]
pub struct RTaskWaker {
    pub id: Uuid,
    pub tasks_tx: Sender<QueuedRTask>,
    pub start_info: RTaskStartInfo,
}

#[derive(Debug)]
pub enum RTaskStatus {
    Started,
    Finished(harp::error::Result<()>),
}

#[derive(Clone)]
pub struct RTaskStartInfo {
    pub thread_id: std::thread::ThreadId,
    pub thread_name: String,
    pub start_time: std::time::Instant,

    /// Time it took to run the time. Used to record time accumulated while
    /// running an async task in the executor. Optional because elapsed time is
    /// computed more simply from start time in other cases.
    pub elapsed_time: Option<std::time::Duration>,

    /// Tracing span for the task
    pub span: tracing::Span,
}

impl QueuedRTask {
    pub(crate) fn start_info_mut(&mut self) -> Option<&mut RTaskStartInfo> {
        match self {
            QueuedRTask::Sync(ref mut task) => Some(&mut task.start_info),
            QueuedRTask::Async(ref mut task) => Some(&mut task.start_info),
            QueuedRTask::Parked(_) => None,
        }
    }
}

// Safety: `RTaskAsync` contains a `!Send` future but is sent through
// crossbeam channels. This is safe because the future is only ever polled
// on the single R thread.
unsafe impl Send for RTaskAsync {}

impl std::task::Wake for RTaskWaker {
    fn wake(self: Arc<RTaskWaker>) {
        let tasks_tx = self.tasks_tx.clone();
        tasks_tx.send(QueuedRTask::Parked(self)).unwrap();
    }
}

impl RTaskStartInfo {
    pub(crate) fn new(idle: bool) -> Self {
        let thread = std::thread::current();
        let thread_id = thread.id();
        let thread_name = thread
            .name()
            .map(|n| n.to_owned())
            .unwrap_or_else(|| format!("{thread_id:?}"))
            .to_owned();

        let start_time = std::time::Instant::now();
        let span = tracing::trace_span!("R task", thread = thread_name, interrupt = !idle,);

        Self {
            thread_id,
            thread_name,
            start_time,
            elapsed_time: None,
            span,
        }
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.elapsed_time
            .unwrap_or_else(|| self.start_time.elapsed())
    }

    pub(crate) fn bump_elapsed(&mut self, duration: Duration) {
        if let Some(ref mut elapsed_time) = self.elapsed_time {
            *elapsed_time += duration;
        }
    }
}

// The `Send` bound on `F` is necessary for safety. Although we are not
// worried about data races since control flow from one thread to the other
// is sequential, objects captured by `f` might have implementations
// sensitive to some thread state (ID, thread-local storage, etc).
//
// The 'env lifetime is for objects captured by the closure `f`.
// `r_task()` is blocking and guaranteed to return _after_ `f` has finished
// running, so borrowing is allowed even though we send it to another
// thread. See also `Crossbeam::thread::ScopedThreadBuilder` (from which
// `r_task()` is adapted) for a similar approach.

pub fn r_task<'env, F, T>(f: F) -> T
where
    F: FnOnce() -> T,
    F: 'env + Send,
    T: 'env + Send,
{
    // Escape hatch for unit tests
    // In integration tests with dummy frontends, we have a "real" Console and want to
    // go through the standard r-task path
    if stdext::IS_TESTING && !Console::is_initialized() {
        let _lock = harp::fixtures::R_TEST_LOCK.lock();
        r_test_init();
        return f();
    }

    // Recursive case: If we're on ark-r-main already, just run the
    // task and return. This allows `r_task(|| { r_task(|| {}) })`
    // to run without deadlocking.
    if Console::on_main_thread() {
        return f();
    }

    // The following is adapted from `Crossbeam::thread::ScopedThreadBuilder`.
    // Instead of scoping the task with a thread join, we send it on the R
    // thread and block the thread until a completion channel wakes us up.

    // The result of `f` will be stored here.
    let result = SharedOption::default();

    {
        let result = Arc::clone(&result);
        let closure = move || {
            *result.lock().unwrap() = Some(f());
        };

        // Move `f` to heap and erase its lifetime so we can send it to
        // another thread. It is safe to do so because we block in this
        // scope until the closure has finished running, so the objects
        // captured by the closure are guaranteed to exist for the duration
        // of the closure call.
        let closure: Box<dyn FnOnce() + Send + 'env> = Box::new(closure);
        let closure: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(closure) };

        // Channel to communicate status of the task/closure
        let (status_tx, status_rx) = bounded::<RTaskStatus>(0);

        // Send the task to the R thread
        let task = QueuedRTask::Sync(RTaskSync {
            fun: closure,
            status_tx: Some(status_tx),
            start_info: RTaskStartInfo::new(false),
        });
        INTERRUPT_TASKS.tx().send(task).unwrap();

        // Block until we get the signal that the task has started
        let status = status_rx.recv().unwrap();

        let RTaskStatus::Started = status else {
            let trace = std::backtrace::Backtrace::force_capture();
            panic!(
                "Task `status` value must be `Started`: {status:?}\n\
                 Backtrace of calling thread:\n\n
                 {trace}"
            );
        };

        // Block until task was completed or timed out
        let status = status_rx.recv().unwrap();

        let RTaskStatus::Finished(status) = status else {
            let trace = std::backtrace::Backtrace::force_capture();
            panic!(
                "Task `status` value must be `Finished`: {status:?}\n\
                 Backtrace of calling thread:\n\n
                 {trace}"
            );
        };

        // If the task failed send a backtrace of the current thread to the
        // main thread
        if let Err(err) = status {
            let trace = std::backtrace::Backtrace::force_capture();
            panic!(
                "While running task: {err:?}\n\
                 Backtrace of calling thread:\n\n\
                 {trace}"
            );
        }
    }

    // Retrieve closure result from the synchronized shared option.
    // If we get here without panicking we know the result was assigned.
    return result.lock().unwrap().take().unwrap();
}

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
        RTask::Idle(Self::pin_with_capture(fun))
    }

    #[allow(unused)]
    pub(crate) fn idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::IdleAnyPrompt(Self::pin_with_capture(fun))
    }

    fn pin_with_capture<F, Fut>(fun: F) -> BoxFuture<'static, ()>
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        Box::pin(async move {
            let capture = if Console::is_initialized() {
                Console::get_mut().start_capture()
            } else {
                // Unit tests run without a Console. The dummy capture is
                // inert and doesn't interact with Console state.
                debug_assert!(stdext::IS_TESTING);
                ConsoleOutputCapture::dummy()
            };
            fun(capture).await
        })
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
        RTask::SendIdle(Self::pin_with_capture(fun))
    }

    pub(crate) fn send_idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdleAnyPrompt(Self::pin_with_capture(fun))
    }
}

/// Spawn an async task on the R thread.
///
/// For `Send` variants (`RTask::send_interrupt`, etc.) this can be called from
/// any thread. Non-`Send` variants must be called from the R thread.
pub(crate) fn spawn(task: RTask) {
    if stdext::IS_TESTING && !Console::is_initialized() {
        let _lock = harp::fixtures::R_TEST_LOCK.lock();
        let fut = match task {
            RTask::Interrupt(fut) |
            RTask::Idle(fut) |
            RTask::IdleAnyPrompt(fut) |
            RTask::SendInterrupt(fut) |
            RTask::SendIdle(fut) |
            RTask::SendIdleAnyPrompt(fut) => fut,
        };
        futures::executor::block_on(fut);
        return;
    }

    let needs_r_thread = matches!(
        task,
        RTask::Interrupt(_) | RTask::Idle(_) | RTask::IdleAnyPrompt(_)
    );
    if needs_r_thread && !Console::on_main_thread() {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        panic!("`spawn()` must be called from the R thread, not thread '{name}'");
    }

    let (fut, tasks_tx, only_idle) = match task {
        RTask::Interrupt(fut) | RTask::SendInterrupt(fut) => (fut, INTERRUPT_TASKS.tx(), false),
        RTask::Idle(fut) | RTask::SendIdle(fut) => (fut, IDLE_TASKS.tx(), true),
        RTask::IdleAnyPrompt(fut) | RTask::SendIdleAnyPrompt(fut) => {
            (fut, IDLE_ANY_TASKS.tx(), true)
        },
    };

    let task = QueuedRTask::Async(RTaskAsync {
        fut,
        tasks_tx: tasks_tx.clone(),
        start_info: RTaskStartInfo::new(only_idle),
    });

    tasks_tx.send(task).unwrap();
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
