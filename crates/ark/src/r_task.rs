//
// r_task.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crossbeam::channel::bounded;
use crossbeam::channel::Sender;
use harp::exec::r_sandbox;
use harp::test::R_TASK_BYPASS;
use stdext::log_and_panic;
use stdext::unwrap;

use crate::interface::RMain;

extern "C" {
    pub static mut R_PolledEvents: Option<unsafe extern "C" fn()>;
}

type SharedOption<T> = Arc<Mutex<Option<T>>>;

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
        let closure: Box<dyn FnOnce() + 'env + Send> = Box::new(closure);
        let closure: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(closure) };

        // Channel to communicate completion status of the task/closure
        let (status_tx, status_rx) = bounded::<harp::error::Result<()>>(0);

        // Send the task to the R thread
        let task = RTaskMain {
            closure: Some(closure),
            status_tx: Some(status_tx),
        };
        get_tasks_tx().send(task).unwrap();

        // Block until task was completed
        let status = status_rx.recv().unwrap();

        // If the task failed send a backtrace of the current thread to the
        // main thread
        if let Err(err) = status {
            let trace = std::backtrace::Backtrace::capture();
            log_and_panic!(
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

pub fn r_async_task<F>(f: F)
where
    F: FnOnce(),
    F: Send + 'static,
{
    // Escape hatch for unit tests
    if unsafe { R_TASK_BYPASS } {
        f();
        return;
    }

    // Recursive case: If we're on ark-r-main already, just run the
    // task and return. This allows `r_task(|| { r_task(|| {}) })`
    // to run without deadlocking.
    if RMain::on_main_thread() {
        f();
        return;
    }

    let thread = std::thread::current();
    let thread_id = thread.id();
    let thread_name = thread.name().unwrap_or("<unnamed>");

    log::info!("Thread '{thread_name}' ({thread_id:?}) is requesting an async task.");

    let closure = move || {
        unwrap!(r_sandbox(f), Err(err) => {
            log_and_panic!("While running async task: {err:?}");
        });
    };

    let closure: Box<dyn FnOnce() + Send + 'static> = Box::new(closure);

    // Send the async task to the R thread
    let task = RTaskMain {
        closure: Some(closure),
        status_tx: None,
    };
    get_tasks_tx().send(task).unwrap();

    // Log that we've sent off the async task
    log::info!("Thread '{thread_name}' ({thread_id:?}) has sent the async task.");
}

pub struct RTaskMain {
    pub closure: Option<Box<dyn FnOnce() + Send + 'static>>,
    pub status_tx: Option<Sender<harp::error::Result<()>>>,
}

impl RTaskMain {
    pub fn fulfill(&mut self) {
        // Move closure here and call it
        let closure = self.closure.take().unwrap();
        let result = r_sandbox(closure);

        match &self.status_tx {
            Some(status_tx) => {
                // Unblock caller via the notification channel if it was a
                // blocking call. Failures are handled by the caller.
                status_tx.send(result.map(|_| ())).unwrap()
            },
            None => {
                // If task is async panic immediately in case of failure
                if let Err(err) = result {
                    log_and_panic!("While running task: {err:?}");
                }
            },
        };
    }
}

/// Channel for sending tasks to `R_MAIN`. Initialized by `initialize()`, but
/// is otherwise only accessed by `r_task()` and `r_async_task()`.
static mut R_MAIN_TASKS_TX: Mutex<Option<Sender<RTaskMain>>> = Mutex::new(None);

pub fn initialize(tasks_tx: Sender<RTaskMain>) {
    unsafe { *R_MAIN_TASKS_TX.lock().unwrap() = Some(tasks_tx) };
}

// Be defensive for the case an auxiliary thread runs a task before R is initialized
// by `start_r()`, which calls `r_task::initialize()`
fn get_tasks_tx() -> Sender<RTaskMain> {
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

        std::thread::sleep(Duration::from_millis(100));

        let elapsed = now.elapsed().unwrap().as_secs();

        if elapsed > 50 {
            panic!("Can't acquire `tasks_tx`.");
        }
    }
}

// Tests are tricky because `harp::test::start_r()` is very bare bones and
// doesn't have an `R_MAIN` or `R_MAIN_TASKS_TX`.
