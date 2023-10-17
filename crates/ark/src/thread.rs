//
// thread.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use harp::test::R_TASK_BYPASS;

use crate::r_task::r_async_task;
use crate::shell::R_MAIN_THREAD_NAME;

/// Private "shelter" around an R object that makes it `Send`able
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
struct RShelter<T: 'static> {
    object: T,
}

unsafe impl<T> Sync for RShelter<T> {}
unsafe impl<T> Send for RShelter<T> {}

/// Thread safe wrapper around a generic R object
///
/// Create one with `new()`, pass it between threads, and access the underlying
/// R object with `get()` once you reach another context that will run on the
/// main R thread. If `get()` is called off the main R thread, it will log an
/// error in release mode and panic in development mode.
///
/// When this object is dropped, it `take()`s the `RShelter` out of the
/// `shelter` and `move`s it to the main R thread through an async task to be
/// able to `drop()` it on the main R thread.
///
/// Purposefully does not implement `Clone`, as we want the thread safe objects
/// to be moved across threads without running any R code.
pub struct RThreadSafe<T: 'static> {
    shelter: Option<RShelter<T>>,
}

impl<T> RThreadSafe<T> {
    pub fn new(object: T) -> Self {
        let shelter = RShelter { object };
        let shelter = Some(shelter);
        Self { shelter }
    }

    /// SAFETY: `get()` can only be called on the main R thread.
    /// We also make an exception for tests where `test::start_r()` is used.
    pub fn get(&self) -> &T {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");

        if name != R_MAIN_THREAD_NAME && unsafe { !R_TASK_BYPASS } {
            #[cfg(debug_assertions)]
            panic!("Can't access thread safe R object on thread '{name}'.");
            #[cfg(not(debug_assertions))]
            log::error!("Can't access thread safe R object on thread '{name}'.");
        }

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

        r_async_task(move || {
            // Run the `drop()` method of the `RShelter`, which in turn
            // runs the `drop()` method of the R object, which uses the R API
            // so it must be called on the main R thread.
            drop(shelter);
        })
    }
}
