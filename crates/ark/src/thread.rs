//
// thread.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use harp::object::RObject;
use harp::test::R_TASK_BYPASS;

use crate::r_task::r_async_task;
use crate::shell::R_MAIN_THREAD_NAME;

/// Private "shelter" around an `RObject` that makes it `Send`able
///
/// Shelters can only be created by `RThreadSafeObject`, and the lifetime
/// management of the `RThreadSafeObject` ensures that the shelter (and the
/// underlying `RObject`) is only dropped on the main R thread (since this uses
/// the R API to unprotect).
///
/// As the `RThreadSafeObject` is dropped, the `RObjectShelter` is _moved_ to
/// the main R thread and dropped there.
struct RObjectShelter {
    object: RObject,
}

unsafe impl Sync for RObjectShelter {}
unsafe impl Send for RObjectShelter {}

/// Thread safe wrapper around an `RObject`
///
/// Create one with `new()`, pass it between threads, and access the underlying
/// R object with `get()` once you reach another context that will run on the
/// main R thread. If `get()` is called off the main R thread, it will log an
/// error in release mode and panic in development mode.
///
/// When this object is dropped, it `take()`s the `RObjectShelter` out of the
/// `shelter` and `move`s it to the main R thread through an async task to be
/// able to `drop()` it on the main R thread.
///
/// Purposefully does not implement `Clone`, as we want the thread safe objects
/// to be moved across threads without running any R code.
pub struct RThreadSafeObject {
    shelter: Option<RObjectShelter>,
}

impl RThreadSafeObject {
    pub fn new(object: RObject) -> Self {
        let shelter = RObjectShelter { object };
        let shelter = Some(shelter);
        Self { shelter }
    }

    /// SAFETY: `get()` can only be called on the main R thread.
    /// We also make an exception for tests where `test::start_r()` is used.
    pub fn get(&self) -> &RObject {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");

        if name != R_MAIN_THREAD_NAME && unsafe { !R_TASK_BYPASS } {
            #[cfg(debug_assertions)]
            panic!("Can't access thread safe `RObject` on thread '{name}'.");
            #[cfg(not(debug_assertions))]
            log::error!("Can't access thread safe `RObject` on thread '{name}'.");
        }

        let shelter: &RObjectShelter = self.shelter.as_ref().unwrap();
        let object: &RObject = &shelter.object;

        object
    }
}

impl Drop for RThreadSafeObject {
    fn drop(&mut self) {
        // Take ownership of the `shelter` and `move` it into the async task
        // to be dropped there
        let shelter = self.shelter.take();

        let Some(shelter) = shelter else {
            log::error!("Can't find a `shelter` in this `RThreadSafeObject`.");
            return;
        };

        r_async_task(move || {
            // Run the `drop()` method of the `RObjectShelter`, which in turn
            // runs the `drop()` method of the `RObject`, which uses the R API
            // so it must be called on the main R thread.
            drop(shelter);
        })
    }
}
