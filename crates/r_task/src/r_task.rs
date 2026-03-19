//
// r_task.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::bounded;

use crate::channels::INTERRUPT_TASKS;
use crate::queue::QueuedTask;
use crate::queue::SyncTaskData;
use crate::queue::TaskStartInfo;
use crate::queue::TaskStatus;
use crate::thread::on_main_thread;

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
    // Escape hatch for unit tests.
    // In integration tests with dummy frontends, we have a "real" event loop
    // consumer and want to go through the standard r-task path.
    #[cfg(feature = "testing")]
    if stdext::IS_TESTING && !crate::thread::is_initialized() {
        let _lock = harp::fixtures::R_TEST_LOCK.lock();
        crate::thread::test_init();
        return f();
    }

    // Recursive case: If we're on the R thread already, just run the
    // task and return. This allows `r_task(|| { r_task(|| {}) })`
    // to run without deadlocking.
    if on_main_thread() {
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
        let (status_tx, status_rx) = bounded::<TaskStatus>(0);

        // Send the task to the R thread
        let task = QueuedTask::Sync(SyncTaskData {
            fun: closure,
            status_tx: Some(status_tx),
            start_info: TaskStartInfo::new(false),
        });
        INTERRUPT_TASKS.tx().send(task).unwrap();

        // Block until we get the signal that the task has started
        let status = status_rx.recv().unwrap();

        let TaskStatus::Started = status else {
            let trace = std::backtrace::Backtrace::force_capture();
            panic!(
                "Task `status` value must be `Started`: {status:?}\n\
                 Backtrace of calling thread:\n\n\
                 {trace}"
            );
        };

        // Block until task was completed or timed out
        let status = status_rx.recv().unwrap();

        let TaskStatus::Finished(status) = status else {
            let trace = std::backtrace::Backtrace::force_capture();
            panic!(
                "Task `status` value must be `Finished`: {status:?}\n\
                 Backtrace of calling thread:\n\n\
                 {trace}"
            );
        };

        // If the task failed send a backtrace of the current thread to the
        // main thread
        if let Err(err) = status {
            let trace = std::backtrace::Backtrace::force_capture();
            panic!(
                "While running task: {err}\n\
                 Backtrace of calling thread:\n\n\
                 {trace}"
            );
        }
    }

    // Retrieve closure result from the synchronized shared option.
    // If we get here without panicking we know the result was assigned.
    let x = result.lock().unwrap().take().unwrap();
    x
}
