//
// thread.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use harp::test::R_TASK_BYPASS;

use crate::interface::RMain;
use crate::r_task;

/// Private "shelter" around a Rust object (typically wrapping a `SEXP`, like
/// an `RObject`) that makes it `Send`able
///
/// Shelters can only be created by `RThreadSafe`, and the lifetime
/// management of the `RThreadSafe` ensures that the shelter (and the
/// underlying R object) is only dropped on the main R thread (since this uses
/// the R API to unprotect).
///
/// As the `RThreadSafe` object is dropped, the `RShelter` is _moved_ to
/// the main R thread and dropped there.
///
/// `T` must have a static lifetime, which seems to enforce that `T` "lives
/// however long we need it to", i.e. it prevents `T` from being a reference
/// to some other object that could get dropped out from under us. I think this
/// effectively means that `RShelter` gets to own the type that it shelters.
/// Without this, we can't move the `RShelter` to the main R thread, since `T`
/// "might not live long enough" according to the compiler.
#[derive(Debug)]
struct RShelter<T: 'static> {
    object: T,
}

unsafe impl<T> Sync for RShelter<T> {}
unsafe impl<T> Send for RShelter<T> {}

/// Thread safe wrapper around a Rust object (typically wrapping a `SEXP`)
///
/// Create one with `new()`, pass it between threads, and access the underlying
/// object with `get()` once you reach another context that will run on the
/// main R thread.
///
/// Both `new()` and `get()` must be called on the main R thread. This ensures
/// that R thread-safe objects can only be created on and unwrapped from the
/// R thread. If either of these are called off the main R thread, they will
/// log an error in release mode and panic in development mode.
///
/// When this object is dropped, it `take()`s the `RShelter` out of the
/// `shelter` and `move`s it to the main R thread through an async task to be
/// able to `drop()` it on the main R thread.
///
/// Purposefully does not implement `Clone`, as we want the thread safe objects
/// to be moved across threads without running any R code.
#[derive(Debug)]
pub struct RThreadSafe<T: 'static> {
    shelter: Option<RShelter<T>>,
}

impl<T> RThreadSafe<T> {
    pub fn new(object: T) -> Self {
        check_on_main_r_thread("new");
        let shelter = RShelter { object };
        let shelter = Some(shelter);
        Self { shelter }
    }

    pub fn get(&self) -> &T {
        check_on_main_r_thread("get");
        let shelter: &RShelter<T> = self.shelter.as_ref().unwrap();
        let object: &T = &shelter.object;
        object
    }
}

impl<T> Drop for RThreadSafe<T> {
    fn drop(&mut self) {
        // Take ownership of the `shelter` and `move` it into the async task
        // to be dropped there
        let shelter = self.shelter.take();

        let Some(shelter) = shelter else {
            log::error!("Can't find a `shelter` in this `RThreadSafe`.");
            return;
        };

        r_task::spawn(|| async move {
            // Run the `drop()` method of the `RShelter`, which in turn
            // runs the `drop()` method of the wrapped Rust object, which likely
            // uses the R API (i.e. if it is an `RObject`) so it must be called
            // on the main R thread.
            drop(shelter);
        })
    }
}

fn check_on_main_r_thread(f: &str) {
    // An exception is made for testing, where we set `R_TASK_BYPASS` inside of
    // `test::start_r()`
    if !RMain::on_main_thread() && unsafe { !R_TASK_BYPASS } {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let message =
            format!("Must call `RThreadSafe::{f}()` on the main R thread, not thread '{name}'.");
        #[cfg(debug_assertions)]
        panic!("{message}");
        #[cfg(not(debug_assertions))]
        log::error!("{message}");
    }
}
