//
// r_task.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::collections::VecDeque;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::RecvTimeoutError;
use crossbeam::channel::Sender;
use crossbeam::channel::TrySendError;
#[cfg(debug_assertions)]
use libr::SEXP;
use uuid::Uuid;

use crate::console::Console;
use crate::console::ConsoleOutputCapture;
use crate::fixtures::r_test_init;
use crate::panic;
use crate::panic::Recovery;

/// Task channels for idle-time tasks (top-level only)
static IDLE_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

/// Task channels for idle tasks that run at any idle prompt (top-level or browser, but
/// not input)
static IDLE_ANY_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

/// Queue for `try_r_task()`. `Console::read_console()` registers each prompt
/// here with a `TryTaskFrame`.
pub(crate) static TRY_TASKS: LazyLock<TryTasks> = LazyLock::new(TryTasks::new);

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

/// Returns receivers for idle and idle-any tasks, and the wake-up channel for
/// try tasks.
/// Initializes the task channels if they haven't been initialized yet.
/// Can only be called once (intended for `Console` during init).
pub(crate) fn take_receivers() -> (Receiver<QueuedRTask>, Receiver<QueuedRTask>, Receiver<()>) {
    (
        IDLE_TASKS.take_rx(),
        IDLE_ANY_TASKS.take_rx(),
        TRY_TASKS.take_wake_rx(),
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
    /// A try task was dropped without running because R left its prompt
    /// first. Sent after the closure has been dropped.
    Rejected,
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
    pub(crate) fn new() -> Self {
        let thread = std::thread::current();
        let thread_id = thread.id();
        let thread_name = thread
            .name()
            .map(|n| n.to_owned())
            .unwrap_or_else(|| format!("{thread_id:?}"))
            .to_owned();

        let start_time = std::time::Instant::now();
        let span = tracing::trace_span!("R task", thread = thread_name);

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
//
// `r_task()`s run via `IDLE_ANY_TASKS`, i.e. they run at top level, and at a debugger
// prompt (but notably not the input prompt as a way to reduce risk of reentrancy).

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

    // Channel to communicate status of the task/closure
    let (status_tx, status_rx) = bounded::<RTaskStatus>(0);

    // SAFETY: We block below until the task has finished running.
    let (task, result) = unsafe { erase_task(f, status_tx) };

    // Send the task to the R thread
    IDLE_ANY_TASKS.tx().send(QueuedRTask::Sync(task)).unwrap();

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

    join_task(&status_rx, &result)
}

/// Run `f` on the R thread if R is at a top-level or browser prompt, without
/// queueing behind user code.
///
/// Returns `None` if R is busy: it is evaluating code or waiting at an input
/// prompt, it left its prompt before `f` could start, or `f` did not start
/// within `start_within`. A long-running idle task occupying the event loop
/// is the usual cause of the last case. Once `f` has started, this blocks
/// until it finishes, however long that takes.
///
/// `Console::read_console()` registers each prompt with a `TryTaskFrame`
/// before announcing idleness to frontends. Only the innermost frame can
/// admit `f`. This lets requests triggered by an idle notification, such as
/// a DAP `evaluate` request after `stopped`, queue before the event loop starts.
/// See `TryTasks::submit()` for timeout and rejection behavior.
///
/// Like `r_task()`, `f` may borrow from the caller's stack, Rust panics in `f`
/// are re-raised on the caller's thread, and an R error escaping `f` panics.
/// When queued, `f` runs through `handle_task()` with `R_Interactive` off.
/// When called from the R thread or in unit tests, it runs inline as is.
///
/// Values that must be dropped on every path, such as a `WorldStateSnapshot`
/// that parks salsa writes, should stay on the caller's stack and be
/// borrowed by `f`. If `f` owned them, an R longjump out of `f` would skip
/// their destructor.
pub(crate) fn try_r_task<'env, F, T>(f: F, start_within: Duration) -> Option<T>
where
    F: FnOnce() -> T,
    F: 'env + Send,
    T: 'env + Send,
{
    if !Console::is_initialized() {
        // Escape hatch for unit tests, which run without a Console. Unlike in
        // `r_task()`, this is gated on `cfg!(test)` rather than `IS_TESTING`:
        // integration tests do have a Console, and a cold-start request must
        // not run `f` inline on the caller's thread while R starts up.
        if cfg!(test) {
            let _lock = harp::fixtures::R_TEST_LOCK.lock();
            r_test_init();
            return Some(f());
        }

        // R hasn't reached its first prompt yet
        return None;
    }

    if Console::on_main_thread() {
        return Some(f());
    }

    TRY_TASKS.submit(f, start_within)
}

/// Box `f` as a `RTaskSync` that can be sent to the R thread, with a slot
/// for its outcome.
///
/// # Safety
///
/// The closure's `'env` lifetime is erased so it can cross threads. The
/// caller must not return before the task has finished running or has been
/// dropped, so that whatever `f` borrows outlives it.
unsafe fn erase_task<'env, F, T>(
    f: F,
    status_tx: Sender<RTaskStatus>,
) -> (RTaskSync, SharedOption<std::thread::Result<T>>)
where
    F: FnOnce() -> T,
    F: 'env + Send,
    T: 'env + Send,
{
    // Stores the outcome of `f`. If the caller can recover panics, catch any unwind
    // on the R thread instead of letting it escape through `r_sandbox`'s C frames.
    // The payload is ferried back and re-raised by `join_task()`, on the calling
    // thread.
    //
    // Without a recovering caller, don't install a boundary on the R thread. The
    // production panic hook must abort rather than let the process continue after
    // losing a thread.
    let caller_recovers_panic = panic::recovers_panic();
    let result: SharedOption<std::thread::Result<T>> = SharedOption::default();

    let closure = {
        let result = Arc::clone(&result);
        move || {
            let caught = if caller_recovers_panic {
                panic::catch_unwind_payload(Recovery::Always, f)
            } else {
                Ok(f())
            };
            *result.lock().unwrap() = Some(caught);
        }
    };

    // Move `f` to heap and erase its lifetime so we can send it to another
    // thread. Sound because of the contract documented above.
    let closure: Box<dyn FnOnce() + Send + 'env> = Box::new(closure);
    let closure: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(closure) };

    let task = RTaskSync {
        fun: closure,
        status_tx: Some(status_tx),
        start_info: RTaskStartInfo::new(),
    };

    (task, result)
}

/// Block until a started task has finished, then return its value or
/// re-raise its panic.
fn join_task<T>(
    status_rx: &Receiver<RTaskStatus>,
    result: &SharedOption<std::thread::Result<T>>,
) -> T {
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

    // The closure ran to completion, and an R-level error would have panicked above.
    // If the caller has a recovery boundary, re-raise any Rust panic caught on the
    // R thread. If there is no recovery boundary, the production panic hook aborts
    // before reaching here.
    let caught = result.lock().unwrap().take().unwrap();
    match caught {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Queue for `try_r_task()`, admitting tasks only while the innermost
/// prompt is configured to serve them.
///
/// `Console::read_console()` registers every prompt, including input
/// prompts that do not serve tasks. For example, `readline()` called from a
/// `later` callback makes callers receive `None` immediately, even if the
/// outer event loop serves tasks.
pub(crate) struct TryTasks {
    state: Mutex<TryTasksState>,

    /// Wakes an event loop parked in `Select` after a push. Capacity 1 so
    /// pushes coalesce: the event loop pops a task at the top of every
    /// iteration, so one pending wake-up is enough.
    wake_tx: Sender<()>,
    wake_rx: Mutex<Option<Receiver<()>>>,
}

struct TryTasksState {
    /// One entry per live `TryTaskFrame`, innermost last. `true` if that
    /// frame's event loop serves try tasks. `queue` is empty unless the last
    /// entry is `true`.
    frames: Vec<bool>,
    next_id: u64,
    queue: VecDeque<(u64, RTaskSync)>,
}

impl TryTasksState {
    fn top_serves(&self) -> bool {
        matches!(self.frames.last(), Some(true))
    }

    fn is_top(&self, index: usize) -> bool {
        index + 1 == self.frames.len()
    }
}

impl TryTasks {
    fn new() -> Self {
        let (wake_tx, wake_rx) = bounded::<()>(1);
        Self {
            state: Mutex::new(TryTasksState {
                frames: Vec::new(),
                next_id: 0,
                queue: VecDeque::new(),
            }),
            wake_tx,
            wake_rx: Mutex::new(Some(wake_rx)),
        }
    }

    fn take_wake_rx(&self) -> Receiver<()> {
        let mut rx = self.wake_rx.lock().unwrap();
        rx.take().expect("`take_wake_rx()` can only be called once")
    }

    /// Register the innermost prompt, with `serving` indicating whether its
    /// event loop serves try tasks. A non-serving frame rejects queued tasks
    /// because the outer event loop cannot resume until this prompt returns.
    pub(crate) fn frame(&self, serving: bool) -> TryTaskFrame<'_> {
        let mut state = self.state.lock().unwrap();
        state.frames.push(serving);
        let index = state.frames.len() - 1;
        Self::reject_unserved(state);

        TryTaskFrame {
            tasks: self,
            index,
            _not_send: PhantomData,
        }
    }

    /// Queue `f` for the event loop of the innermost frame and wait for it.
    /// Returns `None` without waiting if that frame doesn't serve try tasks.
    ///
    /// When `start_within` expires, the caller reclaims a still-queued task
    /// and drops it on its own thread. This releases captured values such as
    /// `WorldStateSnapshot`, which blocks Salsa writes until dropped. If the
    /// event loop has already taken the task to run it, the caller waits for
    /// completion. If the task was taken for rejection, the caller waits for
    /// its closure to be dropped before receiving `None`. Queued tasks are
    /// rejected when the innermost frame stops serving them.
    pub(crate) fn submit<'env, F, T>(&self, f: F, start_within: Duration) -> Option<T>
    where
        F: FnOnce() -> T,
        F: 'env + Send,
        T: 'env + Send,
    {
        self.submit_with(f, |status_rx| status_rx.recv_timeout(start_within))
    }

    /// `submit()` with the wait for the first status injected, so tests can
    /// decide when the timeout fires relative to the R thread popping the
    /// task.
    fn submit_with<'env, F, T>(
        &self,
        f: F,
        wait_for_start: impl FnOnce(&Receiver<RTaskStatus>) -> Result<RTaskStatus, RecvTimeoutError>,
    ) -> Option<T>
    where
        F: FnOnce() -> T,
        F: 'env + Send,
        T: 'env + Send,
    {
        // Room for both `Started` and `Finished` so the R thread never blocks on
        // a send. After a timeout, the caller first tries to reclaim the task
        // from the queue and only then receives, so a rendezvous channel would
        // stall the R thread in the meantime.
        let (status_tx, status_rx) = bounded::<RTaskStatus>(2);

        // SAFETY: Every path below returns only after the task has finished
        // running or has been dropped. `push()` and `remove()` hand a task that
        // never started back to us. The R thread drops a rejected task's closure
        // before sending `Rejected`, and a `Disconnected` status channel means the
        // whole task was dropped.
        let (task, result) = unsafe { erase_task(f, status_tx) };

        let id = match self.push(task) {
            Ok(id) => id,
            Err(task) => {
                drop(task);
                return None;
            },
        };

        let status = match wait_for_start(&status_rx) {
            Ok(status) => status,
            Err(RecvTimeoutError::Timeout) => {
                if let Some(task) = self.remove(id) {
                    drop(task);
                    return None;
                }

                // The R thread took the task off the queue, either to run it or
                // to reject it. Both send a status.
                match status_rx.recv() {
                    Ok(status) => status,
                    Err(err) => {
                        log::error!("`try_r_task()`: Task dropped without a status: {err:?}");
                        return None;
                    },
                }
            },
            Err(RecvTimeoutError::Disconnected) => {
                log::error!("`try_r_task()`: Task dropped without a status");
                return None;
            },
        };

        match status {
            RTaskStatus::Started => Some(join_task(&status_rx, &result)),
            RTaskStatus::Rejected => None,
            RTaskStatus::Finished(_) => {
                let trace = std::backtrace::Backtrace::force_capture();
                panic!(
                    "Task `status` value must be `Started` or `Rejected`: {status:?}\n\
                 Backtrace of calling thread:\n\n
                 {trace}"
                );
            },
        }
    }

    /// Queue `task` if the innermost frame serves try tasks. Otherwise hand it
    /// back so the caller drops it on its own thread.
    fn push(&self, task: RTaskSync) -> Result<u64, RTaskSync> {
        let id = {
            let mut state = self.state.lock().unwrap();
            if !state.top_serves() {
                return Err(task);
            }
            let id = state.next_id;
            state.next_id += 1;
            state.queue.push_back((id, task));
            id
        };

        match self.wake_tx.try_send(()) {
            // `Full` means a wake-up is already pending
            Ok(()) | Err(TrySendError::Full(())) => {},
            Err(TrySendError::Disconnected(())) => {
                log::error!("`try_r_task()`: Wake-up channel is disconnected");
            },
        }

        Ok(id)
    }

    /// Reclaim a task that the R thread hasn't taken off the queue yet.
    fn remove(&self, id: u64) -> Option<RTaskSync> {
        let mut state = self.state.lock().unwrap();
        let index = state.queue.iter().position(|(task_id, _)| *task_id == id)?;
        state.queue.remove(index).map(|(_, task)| task)
    }

    /// Reject queued tasks when the innermost frame does not serve them or
    /// no frame remains, without waiting for `start_within` to expire. Called
    /// after every frame-stack change.
    ///
    /// Consumes the guard to release the lock before dropping task closures.
    /// Their destructors may run arbitrary code and reacquire this
    /// non-reentrant mutex.
    fn reject_unserved(mut state: MutexGuard<'_, TryTasksState>) {
        if state.top_serves() {
            return;
        }
        let rejected = std::mem::take(&mut state.queue);
        drop(state);

        for (_, task) in rejected {
            let RTaskSync { fun, status_tx, .. } = task;

            // The closure may borrow from the caller's stack. Drop it before
            // `Rejected` lets the caller return.
            drop(fun);

            if let Some(status_tx) = status_tx {
                if let Err(err) = status_tx.send(RTaskStatus::Rejected) {
                    log::error!("`try_r_task()`: Can't reject task: {err:?}");
                }
            }
        }
    }
}

/// One prompt in the `TryTasks` stack, registered by `TryTasks::frame()`
/// and removed on drop. Only the innermost frame admits and serves tasks.
/// Dropping it preserves queued tasks if the outer frame serves them,
/// otherwise it rejects them.
///
/// Frames must be created and dropped in LIFO order on the R thread.
/// `!Send` prevents moving a frame to another thread.
pub(crate) struct TryTaskFrame<'a> {
    tasks: &'a TryTasks,
    index: usize,
    _not_send: PhantomData<*const ()>,
}

impl TryTaskFrame<'_> {
    /// Stop serving try tasks at this prompt, rejecting those queued. Must be
    /// called on the innermost frame: rejection only looks at the top.
    pub(crate) fn suspend(&self) {
        let mut state = self.tasks.state.lock().unwrap();
        debug_assert!(state.is_top(self.index));
        if let Some(serving) = state.frames.get_mut(self.index) {
            *serving = false;
        }
        TryTasks::reject_unserved(state);
    }

    /// Whether this prompt's event loop serves try tasks.
    pub(crate) fn serves(&self) -> bool {
        let state = self.tasks.state.lock().unwrap();
        matches!(state.frames.get(self.index), Some(true))
    }

    /// Take the next try task off the queue, to be run with `handle_task()`.
    /// Returns `None` unless this frame is the innermost one and serves.
    /// Taking a task off the queue commits to running it. Its caller can no
    /// longer reclaim it.
    pub(crate) fn pop(&self) -> Option<QueuedRTask> {
        let mut state = self.tasks.state.lock().unwrap();
        if !state.is_top(self.index) || !state.top_serves() {
            return None;
        }
        state
            .queue
            .pop_front()
            .map(|(_, task)| QueuedRTask::Sync(task))
    }
}

impl Drop for TryTaskFrame<'_> {
    fn drop(&mut self) {
        let mut state = self.tasks.state.lock().unwrap();
        debug_assert!(state.is_top(self.index));

        // Entries above this one remain only if their frame's destructor was
        // skipped, e.g. by an R longjump. Debug builds catch that with the
        // assertion above; in release builds `truncate()` discards them too.
        state.frames.truncate(self.index);
        TryTasks::reject_unserved(state);
    }
}

/// An async task to be run on the R thread.
///
/// Construct via [RTask::idle] or [RTask::idle_any_prompt] when spawning from the R
/// thread. Use the `Send` variants ([RTask::send_idle], etc.) when spawning from other
/// threads.
///
/// Console output is automatically captured during the task's execution via a
/// `ConsoleOutputCapture` passed to the closure.
pub(crate) enum RTask {
    /// Run when R is at a top-level idle prompt. Must be spawned from the R thread.
    Idle(BoxFuture<'static, ()>),
    /// Run when R is at any idle prompt (top-level or browser). Must be spawned
    /// from the R thread.
    IdleAnyPrompt(BoxFuture<'static, ()>),
    /// Like `Idle`, but can be spawned from any thread. The constructor
    /// enforces `Send` on the closure.
    SendIdle(BoxFuture<'static, ()>),
    /// Like `IdleAnyPrompt`, but can be spawned from any thread. The constructor
    /// enforces `Send` on the closure.
    SendIdleAnyPrompt(BoxFuture<'static, ()>),
}

impl RTask {
    pub(crate) fn idle<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::Idle(Self::pin_with_capture(fun))
    }

    #[expect(unused)]
    pub(crate) fn idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::IdleAnyPrompt(Self::pin_with_capture(fun))
    }

    /// [Self::idle_any_prompt()], but without capture support
    ///
    /// Can be useful for spawning long running event loops that don't emit R output to
    /// avoid [Self::pin_with_capture()]'s behavior of setting `options(warn = 1)` for the
    /// life of `fun`.
    pub(crate) fn idle_any_prompt_without_capture<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::IdleAnyPrompt(Box::pin(fun()))
    }

    pub(crate) fn send_idle<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdle(Self::pin_with_capture(fun))
    }

    /// For rare performance sensitive cases where you'd like to avoid the cost of
    /// poking R options via [Self::pin_with_capture()] and you know you don't need
    /// the safety of capturing output because you aren't running R code
    pub(crate) fn send_idle_without_capture<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdle(Box::pin(fun()))
    }

    pub(crate) fn send_idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce(ConsoleOutputCapture) -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdleAnyPrompt(Self::pin_with_capture(fun))
    }

    // Note that `start_capture()` sets `options(warn = 1)` and this persists
    // for the life of `f`, so avoid using capturing on long running event loops
    // that don't emit R output
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
}

/// Spawn an async task on the R thread.
///
/// For `Send` variants ([RTask::send_idle], etc.) this can be called from
/// any thread. Non-`Send` variants must be called from the R thread.
pub(crate) fn spawn(task: RTask) {
    if stdext::IS_TESTING && !Console::is_initialized() {
        let _lock = harp::fixtures::R_TEST_LOCK.lock();
        let fut = match task {
            RTask::Idle(fut) |
            RTask::IdleAnyPrompt(fut) |
            RTask::SendIdle(fut) |
            RTask::SendIdleAnyPrompt(fut) => fut,
        };
        futures::executor::block_on(fut);
        return;
    }

    let needs_r_thread = matches!(task, RTask::Idle(_) | RTask::IdleAnyPrompt(_));
    if needs_r_thread && !Console::on_main_thread() {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        panic!("`spawn()` must be called from the R thread, not thread '{name}'");
    }

    let (fut, tasks_tx) = match task {
        RTask::Idle(fut) | RTask::SendIdle(fut) => (fut, IDLE_TASKS.tx()),
        RTask::IdleAnyPrompt(fut) | RTask::SendIdleAnyPrompt(fut) => (fut, IDLE_ANY_TASKS.tx()),
    };

    let task = QueuedRTask::Async(RTaskAsync {
        fut,
        tasks_tx: tasks_tx.clone(),
        start_info: RTaskStartInfo::new(),
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

/// Spawn an idle task that evaluates an R expression. Used in integration tests.
#[cfg(debug_assertions)]
#[harp::register]
unsafe extern "C-unwind" fn ps_test_spawn_eval_idle_task(code: SEXP) -> anyhow::Result<SEXP> {
    stdext::assert_testing();

    let code: String = harp::RObject::view(code).try_into()?;

    spawn(RTask::idle(async move |_capture| {
        if let Err(err) = harp::parse_eval_global(&code) {
            log::error!("Idle task eval failed: {err:?}");
        }
    }));

    Ok(libr::R_NilValue)
}

// Protocol tests for `TryTasks`, without R. A scoped thread stands in for
// the R thread, registering prompts and running tasks with `run_task()`.
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::thread::ThreadId;
    use std::time::Duration;

    use crossbeam::channel::bounded;
    use crossbeam::channel::Receiver;
    use crossbeam::channel::RecvTimeoutError;

    use super::QueuedRTask;
    use super::RTaskStatus;
    use super::TryTaskFrame;
    use super::TryTasks;
    use crate::panic;
    use crate::panic::Recovery;

    /// Long enough that the frame stack or the task decides the outcome,
    /// never `start_within`.
    const NEVER: Duration = Duration::from_secs(60);

    /// Records which thread drops it, to observe where and when a closure
    /// capturing it is dropped.
    struct DropProbe(Arc<Mutex<Option<ThreadId>>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = Some(std::thread::current().id());
        }
    }

    fn drop_probe() -> (DropProbe, Arc<Mutex<Option<ThreadId>>>) {
        let dropped_on = Arc::new(Mutex::new(None));
        (DropProbe(Arc::clone(&dropped_on)), dropped_on)
    }

    /// Runs a popped task like `Console::handle_task_inner()` does.
    fn run_task(task: QueuedRTask) {
        let QueuedRTask::Sync(task) = task else {
            panic!("Expected a sync task");
        };
        let status_tx = task.status_tx.unwrap();
        status_tx.send(RTaskStatus::Started).unwrap();
        (task.fun)();
        status_tx.send(RTaskStatus::Finished(Ok(()))).unwrap();
    }

    /// Waits for a wake-up and pops from `frame`, like the event loop.
    fn pop_next(frame: &TryTaskFrame, wake_rx: &Receiver<()>) -> QueuedRTask {
        loop {
            wake_rx.recv().unwrap();
            if let Some(task) = frame.pop() {
                return task;
            }
        }
    }

    #[test]
    fn test_try_task_refused_without_frame() {
        let tasks = TryTasks::new();
        assert_eq!(tasks.submit(|| 1, NEVER), None);
    }

    #[test]
    fn test_try_task_refused_when_top_does_not_serve() {
        let tasks = TryTasks::new();
        let _outer = tasks.frame(true);
        let _inner = tasks.frame(false);
        assert_eq!(tasks.submit(|| 1, NEVER), None);
    }

    #[test]
    fn test_try_task_served_with_borrowed_data() {
        let tasks = &TryTasks::new();
        let wake_rx = tasks.take_wake_rx();
        let data = [1, 2, 3];

        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = bounded::<()>(0);
            scope.spawn(move || {
                let frame = tasks.frame(true);
                ready_tx.send(()).unwrap();
                run_task(pop_next(&frame, &wake_rx));
            });

            ready_rx.recv().unwrap();
            let sum = tasks.submit(|| data.iter().sum::<i32>(), NEVER);
            assert_eq!(sum, Some(6));
        });
    }

    #[test]
    fn test_try_task_timeout_while_queued_drops_on_caller() {
        let tasks = &TryTasks::new();

        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = bounded::<()>(0);
            let (done_tx, done_rx) = bounded::<()>(0);
            scope.spawn(move || {
                // Keep the task queued until the caller reclaims it on timeout.
                let frame = tasks.frame(true);
                ready_tx.send(()).unwrap();
                done_rx.recv().unwrap();
                assert!(frame.pop().is_none());
            });

            ready_rx.recv().unwrap();
            let (probe, dropped_on) = drop_probe();
            let out = tasks.submit(
                move || {
                    drop(probe);
                    1
                },
                Duration::from_millis(10),
            );
            assert_eq!(out, None);
            assert_eq!(
                *dropped_on.lock().unwrap(),
                Some(std::thread::current().id())
            );
            done_tx.send(()).unwrap();
        });
    }

    #[test]
    fn test_try_task_timeout_after_pop_still_runs() {
        let tasks = &TryTasks::new();
        let wake_rx = tasks.take_wake_rx();

        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = bounded::<()>(0);
            let (popped_tx, popped_rx) = bounded::<()>(0);
            scope.spawn(move || {
                let frame = tasks.frame(true);
                ready_tx.send(()).unwrap();
                let task = pop_next(&frame, &wake_rx);
                popped_tx.send(()).unwrap();
                run_task(task);
            });

            ready_rx.recv().unwrap();

            // Time out only once the task has been popped, so the caller's
            // reclaim fails and it must wait for the task to run
            let out = tasks.submit_with(
                || 1,
                |_| {
                    popped_rx.recv().unwrap();
                    Err(RecvTimeoutError::Timeout)
                },
            );
            assert_eq!(out, Some(1));
        });
    }

    #[test]
    fn test_try_task_rejected_when_last_frame_dropped() {
        let tasks = &TryTasks::new();
        let wake_rx = tasks.take_wake_rx();

        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = bounded::<()>(0);
            let r_thread = scope.spawn(move || {
                let frame = tasks.frame(true);
                ready_tx.send(()).unwrap();
                wake_rx.recv().unwrap();
                drop(frame);
            });
            let r_thread_id = r_thread.thread().id();

            ready_rx.recv().unwrap();
            let (probe, dropped_on) = drop_probe();
            let out = tasks.submit(
                move || {
                    drop(probe);
                    1
                },
                NEVER,
            );
            assert_eq!(out, None);
            assert_eq!(*dropped_on.lock().unwrap(), Some(r_thread_id));
        });
    }

    #[test]
    fn test_try_task_kept_when_nested_frame_dropped_over_serving_frame() {
        let tasks = &TryTasks::new();
        let wake_rx = tasks.take_wake_rx();

        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = bounded::<()>(0);
            scope.spawn(move || {
                let outer = tasks.frame(true);
                let inner = tasks.frame(true);
                ready_tx.send(()).unwrap();
                wake_rx.recv().unwrap();

                // A serving outer frame cannot take tasks from an inner prompt.
                assert!(outer.pop().is_none());
                drop(inner);

                let task = outer.pop().unwrap();
                run_task(task);
            });

            ready_rx.recv().unwrap();
            assert_eq!(tasks.submit(|| 1, NEVER), Some(1));
        });
    }

    #[test]
    fn test_try_task_nested_non_serving_frame_rejects_and_refuses() {
        let tasks = &TryTasks::new();
        let wake_rx = tasks.take_wake_rx();

        std::thread::scope(|scope| {
            let (step_tx, step_rx) = bounded::<()>(0);
            let (resume_tx, resume_rx) = bounded::<()>(0);
            scope.spawn(move || {
                let outer = tasks.frame(true);
                step_tx.send(()).unwrap();

                wake_rx.recv().unwrap();
                let inner = tasks.frame(false);
                step_tx.send(()).unwrap();

                resume_rx.recv().unwrap();
                drop(inner);
                step_tx.send(()).unwrap();

                run_task(pop_next(&outer, &wake_rx));
            });

            // Queued for `outer`, rejected when `inner` is pushed
            step_rx.recv().unwrap();
            assert_eq!(tasks.submit(|| 1, NEVER), None);

            // Refused while `inner` is the innermost frame
            step_rx.recv().unwrap();
            assert_eq!(tasks.submit(|| 2, NEVER), None);
            resume_tx.send(()).unwrap();

            // Served by `outer` once `inner` is gone
            step_rx.recv().unwrap();
            assert_eq!(tasks.submit(|| 3, NEVER), Some(3));
        });
    }

    #[test]
    fn test_try_task_rejected_on_suspend() {
        let tasks = &TryTasks::new();
        let wake_rx = tasks.take_wake_rx();

        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = bounded::<()>(0);
            scope.spawn(move || {
                let frame = tasks.frame(true);
                ready_tx.send(()).unwrap();
                wake_rx.recv().unwrap();

                frame.suspend();
                assert!(!frame.serves());
                assert!(frame.pop().is_none());
            });

            ready_rx.recv().unwrap();
            assert_eq!(tasks.submit(|| 1, NEVER), None);
        });
    }

    #[test]
    fn test_try_task_panic_reraised_on_caller() {
        let tasks = &TryTasks::new();
        let wake_rx = tasks.take_wake_rx();

        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = bounded::<()>(0);
            scope.spawn(move || {
                let frame = tasks.frame(true);
                ready_tx.send(()).unwrap();
                run_task(pop_next(&frame, &wake_rx));
            });

            ready_rx.recv().unwrap();
            let result = panic::catch_unwind(Recovery::Always, || {
                tasks.submit(|| -> i32 { panic!("boom") }, NEVER)
            });
            assert_eq!(result, Err(String::from("boom")));
        });
    }
}
