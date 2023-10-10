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
use harp::exec::safely;
use harp::test::R_TASK_BYPASS;
use log::info;

use crate::interface::RMain;
use crate::interface::R_MAIN;

extern "C" {
    pub static mut R_PolledEvents: Option<unsafe extern "C" fn()>;
}

type SharedOption<T> = Arc<Mutex<Option<T>>>;

pub fn r_task<'env, F, T>(f: F) -> T
where
    F: FnOnce() -> T,
    F: 'env,
    T: 'env,
{
    // Escape hatch for unit tests
    if unsafe { R_TASK_BYPASS } {
        return f();
    }

    let main = acquire_r_main();

    // Recursive case: If we're on ark-r-main already, just run the
    // task and return. This allows `r_task(|| { r_task(|| {}) })`
    // to run without deadlocking.
    let thread_id = std::thread::current().id();
    if main.thread_id == thread_id {
        return f();
    }

    // The following is adapted from `Crossbeam::thread::ScopedThreadBuilder`.
    // Instead of scoping the task with a thread join, we send it on the R
    // thread and block the thread until a completion channel wakes us up.

    // Record how long it takes for the task to be picked up on the main thread.
    let now = std::time::SystemTime::now();

    // The result of `f` will be stored here.
    let result = SharedOption::default();
    let result = Arc::clone(&result);

    {
        let result = Arc::clone(&result);
        let closure = move || {
            let res = safely(f);
            *result.lock().unwrap() = Some(res);
        };

        // Move `f` to heap and erase its lifetime
        let closure: Box<dyn FnOnce() + 'env> = Box::new(closure);
        let closure: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(closure) };

        // Channel to communicate completion status of the task/closure
        let (status_tx, status_rx) = bounded::<bool>(0);

        // Send the task to the R thread
        let task = RTaskMain {
            closure: Some(closure),
            status_tx,
        };
        main.tasks_tx.send(task).unwrap();

        // Block until task was completed
        status_rx.recv().unwrap();
    }

    // Log how long we were stuck waiting.
    let elapsed = now.elapsed().unwrap().as_millis();
    info!(
        "Thread '{}' ({:?}) was unblocked after waiting for {} milliseconds.",
        std::thread::current().name().unwrap_or("<unnamed>"),
        thread_id,
        elapsed
    );

    // Retrieve closure result from the synchronized shared option.
    // If we get here without panicking we know the result was assigned.
    return result.lock().unwrap().take().unwrap();
}

pub struct RTaskMain {
    pub closure: Option<Box<dyn FnOnce() + Send + 'static>>,
    pub status_tx: crossbeam::channel::Sender<bool>,
}

impl RTaskMain {
    pub fn fulfill(&mut self) {
        // Move closure here and call it
        self.closure.take().map(|closure| closure());

        // Unblock caller
        self.status_tx.send(true).unwrap();
    }
}

// Be defensive for the case an auxiliary thread runs a task before R is initialized
fn acquire_r_main() -> &'static mut RMain {
    let now = std::time::SystemTime::now();

    unsafe {
        loop {
            if !R_MAIN.is_none() {
                return R_MAIN.as_mut().unwrap();
            }
            std::thread::sleep(Duration::from_millis(100));

            let elapsed = now.elapsed().unwrap().as_secs();
            if elapsed > 50 {
                panic!("Can't acquire main thread");
            }
        }
    }
}

// Tests are tricky because `harp::test::start_r()` is very bare bones and
// doesn't have an `R_MAIN`.
