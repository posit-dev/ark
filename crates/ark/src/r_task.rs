//
// r_task.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::{Arc, Mutex};

use crossbeam::channel::bounded;

use crate::interface::R_MAIN;

type SharedOption<T> = Arc<Mutex<Option<T>>>;

pub fn r_task<'env, F, T>(f: F) -> T
where
    F: FnOnce() -> T,
    F: 'env + Send,
    T: 'env + Send,
{
    // Recursive case: If we're on ark-r-main already, just run the
    // task and return. This allows `r_task(|| { r_task(|| {}) })`
    // to run without deadlocking.
    let main = unsafe { R_MAIN.as_mut().unwrap() }; // FIXME: Check init timings
    if main.thread_id == std::thread::current().id() {
        return f();
    }

    // The following is adapted from `Crossbeam::thread::ScopedThreadBuilder`.
    // Instead of scoping the task with a thread join, we send it on the R
    // thread and block the thread until a completion channel wakes us up.

    // The result of `f` will be stored here.
    let result = SharedOption::default();
    let result = Arc::clone(&result);

    {
        let result = Arc::clone(&result);
        let closure = move || {
            let res = f();
            *result.lock().unwrap() = Some(res);
        };

        // Move `f` to heap and erase its lifetime
        let closure: Box<dyn FnOnce() + Send + 'env> = Box::new(closure);
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

// Tests are tricky because `harp::test::start_r()` is very bare bones and
// doesn't have an `R_MAIN`.
