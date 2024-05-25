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

pub enum RTaskKind {
    Closure(Box<dyn FnOnce() + Send + Sync + 'static>),
    Future(BoxFuture<'static, ()>),
    ParkedTask(Uuid),
}

// RTaskKind is not Send because of the Future variant which doesn't require
// Send to avoid issues across await points, but the future as a whole is
// actually safe to send to other threads.
unsafe impl Send for RTaskKind {}

#[derive(Debug)]
pub enum RTaskStatus {
    Started,
    Finished(harp::error::Result<()>),
}

pub struct RTask {
    pub task: RTaskKind,
    pub status_tx: Option<Sender<RTaskStatus>>,
    pub only_idle: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RTaskWaker {
    pub(crate) task_id: Uuid,
    pub(crate) status_tx: Option<Sender<RTaskStatus>>,
    pub(crate) tasks_tx: Sender<RTask>,
    pub(crate) only_idle: bool,
}

impl std::task::Wake for RTaskWaker {
    fn wake(self: Arc<RTaskWaker>) {
        let task = RTask {
            task: RTaskKind::ParkedTask(self.task_id),
            status_tx: self.status_tx.clone(),
            only_idle: self.only_idle,
        };
        self.tasks_tx.send(task).unwrap();
    }
}

/// Channel for sending tasks to `R_MAIN`. Initialized by `initialize()`, but
/// is otherwise only accessed by `r_task()` and `r_async_task()`.
static mut R_MAIN_TASKS_TX: Mutex<Option<Sender<RTask>>> = Mutex::new(None);

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
    F: 'env + Send + Sync,
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

    let thread = std::thread::current();
    let thread_id = thread.id();
    let thread_name = thread.name().unwrap_or("<unnamed>");

    log::info!("Thread '{thread_name}' ({thread_id:?}) is requesting a task.");

    // The following is adapted from `Crossbeam::thread::ScopedThreadBuilder`.
    // Instead of scoping the task with a thread join, we send it on the R
    // thread and block the thread until a completion channel wakes us up.

    // Record how long it takes for the task to be picked up on the main thread.
    let now = std::time::SystemTime::now();

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
        let closure: Box<dyn FnOnce() + Send + Sync + 'env> = Box::new(closure);
        let closure: Box<dyn FnOnce() + Send + Sync + 'static> =
            unsafe { std::mem::transmute(closure) };

        // Channel to communicate status of the task/closure
        let (status_tx, status_rx) = bounded::<RTaskStatus>(0);

        // Send the task to the R thread
        let task = RTask {
            task: RTaskKind::Closure(closure),
            status_tx: Some(status_tx),
            only_idle: false,
        };
        get_tasks_tx().send(task).unwrap();

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

    // Log how long we were stuck waiting.
    let elapsed = now.elapsed().unwrap().as_millis();
    log::info!(
        "Thread '{thread_name}' ({thread_id:?}) was unblocked after waiting for {elapsed} milliseconds."
    );

    // Retrieve closure result from the synchronized shared option.
    // If we get here without panicking we know the result was assigned.
    return result.lock().unwrap().take().unwrap();
}

pub fn spawn<F, Fut>(fun: F)
where
    F: FnOnce() -> Fut + 'static + Send,
    Fut: Future<Output = ()> + 'static,
{
    spawn_ext(fun, false)
}

pub fn spawn_idle<F, Fut>(fun: F)
where
    F: FnOnce() -> Fut + 'static + Send,
    Fut: Future<Output = ()> + 'static,
{
    spawn_ext(fun, true)
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

    let thread = std::thread::current();
    let thread_id = thread.id();
    let thread_name = thread.name().unwrap_or("<unnamed>");

    log::trace!("Thread '{thread_name}' ({thread_id:?}) is spawning a task.");

    let fut = Box::pin(fun()) as BoxFuture<'static, ()>;

    // Send the async task to the R thread
    let task = RTask {
        task: RTaskKind::Future(fut),
        status_tx: None,
        only_idle,
    };
    get_tasks_tx().send(task).unwrap();
}

pub fn initialize(tasks_tx: Sender<RTask>) {
    unsafe { *R_MAIN_TASKS_TX.lock().unwrap() = Some(tasks_tx) };
}

// Be defensive for the case an auxiliary thread runs a task before R is initialized
// by `start_r()`, which calls `r_task::initialize()`
fn get_tasks_tx() -> Sender<RTask> {
    let now = std::time::SystemTime::now();

    loop {
        let guard = unsafe { R_MAIN_TASKS_TX.lock().unwrap() };

        if let Some(ref tasks_tx) = *guard {
            // Return a clone of the sender so we can immediately unlock
            // `R_MAIN_TASKS_TX` for use by other tasks (especially async ones)
            return tasks_tx.clone();
        }

        // If not initialized, drop to give `initialize()` time to lock
        // and set `R_MAIN_TASKS_TX`
        drop(guard);

        log::info!("`tasks_tx` not yet initialized, going to sleep for 100ms.");

        std::thread::sleep(Duration::from_millis(100));

        let elapsed = now.elapsed().unwrap().as_secs();

        if elapsed > 50 {
            panic!("Can't acquire `tasks_tx` after 50 seconds.");
        }
    }
}

pub(crate) fn is_parked_task(task: &RTask) -> bool {
    if let RTaskKind::ParkedTask(_) = task.task {
        true
    } else {
        false
    }
}

// Tests are tricky because `harp::test::start_r()` is very bare bones and
// doesn't have an `R_MAIN` or `R_MAIN_TASKS_TX`.
