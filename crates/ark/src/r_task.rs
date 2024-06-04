//
// r_task.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use crossbeam::channel::bounded;
use crossbeam::channel::Sender;
use harp::test::R_TASK_BYPASS;
use uuid::Uuid;

use crate::interface::RMain;

// Compared to `futures::BoxFuture`, this doesn't require the future to be Send.
// We don't need this bound since the executor runs on only on the R thread
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

type SharedOption<T> = Arc<Mutex<Option<T>>>;

pub enum RTask {
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
    pub tasks_tx: Sender<RTask>,
    pub start_info: RTaskStartInfo,
}

#[derive(Clone)]
pub struct RTaskWaker {
    pub id: Uuid,
    pub tasks_tx: Sender<RTask>,
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

impl RTask {
    pub(crate) fn start_info_mut(&mut self) -> Option<&mut RTaskStartInfo> {
        match self {
            RTask::Sync(ref mut task) => Some(&mut task.start_info),
            RTask::Async(ref mut task) => Some(&mut task.start_info),
            RTask::Parked(_) => None,
        }
    }
}

// RTaskAsync is not Send because of the Future variant which doesn't require
// Send to avoid issues across await points, but the future as a whole is
// actually safe to send to other threads.
unsafe impl Send for RTaskAsync {}
unsafe impl Sync for RTaskAsync {}

impl std::task::Wake for RTaskWaker {
    fn wake(self: Arc<RTaskWaker>) {
        let tasks_tx = self.tasks_tx.clone();
        tasks_tx.send(RTask::Parked(self)).unwrap();
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
            *elapsed_time = *elapsed_time + duration;
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
    if unsafe { R_TASK_BYPASS } {
        return f();
    }

    // Recursive case: If we're on ark-r-main already, just run the
    // task and return. This allows `r_task(|| { r_task(|| {}) })`
    // to run without deadlocking.
    if RMain::on_main_thread() {
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
        let task = RTask::Sync(RTaskSync {
            fun: closure,
            status_tx: Some(status_tx),
            start_info: RTaskStartInfo::new(false),
        });
        get_tasks_interrupt_tx().send(task).unwrap();

        // Block until we get the signal that the task has started
        let status = status_rx.recv().unwrap();

        let RTaskStatus::Started = status else {
            let trace = std::backtrace::Backtrace::capture();
            panic!(
                "Task `status` value must be `Started`: {status:?}\n\
                 Backtrace of calling thread:\n\n
                 {trace}"
            );
        };

        // Block until task was completed or timed out
        let status = status_rx.recv().unwrap();

        let RTaskStatus::Finished(status) = status else {
            let trace = std::backtrace::Backtrace::capture();
            panic!(
                "Task `status` value must be `Finished`: {status:?}\n\
                 Backtrace of calling thread:\n\n
                 {trace}"
            );
        };

        // If the task failed send a backtrace of the current thread to the
        // main thread
        if let Err(err) = status {
            let trace = std::backtrace::Backtrace::capture();
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

#[allow(dead_code)] // Currently unused
pub(crate) fn spawn_idle<F, Fut>(fun: F)
where
    F: FnOnce() -> Fut + 'static + Send,
    Fut: Future<Output = ()> + 'static,
{
    spawn_ext(fun, true)
}

pub(crate) fn spawn_interrupt<F, Fut>(fun: F)
where
    F: FnOnce() -> Fut + 'static + Send,
    Fut: Future<Output = ()> + 'static,
{
    spawn_ext(fun, false)
}

fn spawn_ext<F, Fut>(fun: F, only_idle: bool)
where
    F: FnOnce() -> Fut + 'static + Send,
    Fut: Future<Output = ()> + 'static,
{
    // Idle tasks are always run from the read-console loop
    if !only_idle && unsafe { R_TASK_BYPASS } {
        // Escape hatch for unit tests
        futures::executor::block_on(fun());
        return;
    }

    let tasks_tx = if only_idle {
        get_tasks_idle_tx()
    } else {
        get_tasks_interrupt_tx()
    };

    // Send the async task to the R thread
    let task = RTask::Async(RTaskAsync {
        fut: Box::pin(fun()) as BoxFuture<'static, ()>,
        tasks_tx: tasks_tx.clone(),
        start_info: RTaskStartInfo::new(only_idle),
    });

    tasks_tx.send(task).unwrap();
}

/// Channel for sending tasks to `R_MAIN`. Initialized by `initialize()`, but
/// is otherwise only accessed to create `RTask`s.
static mut R_MAIN_TASKS_INTERRUPT_TX: OnceLock<Sender<RTask>> = OnceLock::new();
static mut R_MAIN_TASKS_IDLE_TX: OnceLock<Sender<RTask>> = OnceLock::new();

pub fn initialize(tasks_tx: Sender<RTask>, tasks_idle_tx: Sender<RTask>) {
    unsafe { R_MAIN_TASKS_INTERRUPT_TX.set(tasks_tx).unwrap() };
    unsafe { R_MAIN_TASKS_IDLE_TX.set(tasks_idle_tx).unwrap() };
}

// Be defensive for the case an auxiliary thread runs a task before R is initialized
// by `start_r()`, which calls `r_task::initialize()`
fn get_tasks_interrupt_tx() -> &'static Sender<RTask> {
    get_tx(unsafe { &R_MAIN_TASKS_INTERRUPT_TX })
}
fn get_tasks_idle_tx() -> &'static Sender<RTask> {
    get_tx(unsafe { &R_MAIN_TASKS_IDLE_TX })
}

fn get_tx(once_tx: &'static OnceLock<Sender<RTask>>) -> &'static Sender<RTask> {
    let now = std::time::Instant::now();

    loop {
        if let Some(tx) = once_tx.get() {
            return tx;
        }

        // Wait for `initialize()`
        log::info!("`tasks_tx` not yet initialized, going to sleep for 50ms.");
        std::thread::sleep(Duration::from_millis(50));

        if now.elapsed().as_secs() > 50 {
            panic!("Can't acquire `tasks_tx` after 50 seconds.");
        }
    }
}

// Tests are tricky because `harp::test::start_r()` is very bare bones and
// doesn't have an `R_MAIN` or `R_MAIN_TASKS_TX`.
