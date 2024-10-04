//
// fixtures/mod.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

// Helper functions for ensuring R is running before running tests
// that rely on an R session being available.

use std::os::raw::c_char;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

use libr::setup_Rmainloop;
use libr::R_CStackLimit;
use libr::Rf_initialize_R;
use stdext::cargs;

use crate::library::RLibraries;
use crate::R_MAIN_THREAD_ID;

// FIXME: Needs to be a reentrant lock for idle tasks. We can probably do better
// though.
pub static mut R_TEST_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());

static INIT: Once = Once::new();

/// Run code accessing the R API in a safe context.
///
/// Takes a lock on `R_TEST_LOCK` and ensures R is initialized.
///
/// Note: `harp::r_task()` should only be used in Harp tests. Use
/// `ark::r_task()` in Ark tests so that Ark initialisation also takes place.
#[cfg(test)]
pub(crate) fn r_task<F: FnOnce()>(f: F) {
    let guard = unsafe { R_TEST_LOCK.lock() };

    r_test_init();
    f();

    drop(guard);
}

pub fn r_test_init() {
    INIT.call_once(|| {
        unsafe {
            R_MAIN_THREAD_ID = Some(std::thread::current().id());
        }

        // Set up R_HOME if necessary.
        let r_home = match std::env::var("R_HOME") {
            Ok(r_home) => PathBuf::from(r_home),
            Err(_) => {
                let result = Command::new("R").arg("RHOME").output().unwrap();
                let r_home = String::from_utf8(result.stdout).unwrap();
                let r_home = r_home.trim();
                unsafe { std::env::set_var("R_HOME", r_home) };
                PathBuf::from(r_home)
            },
        };

        let libraries = RLibraries::from_r_home_path(&r_home);
        libraries.initialize_pre_setup_r();

        r_test_setup();

        libraries.initialize_post_setup_r();

        // Initialize harp globals
        unsafe {
            crate::routines::r_register_routines();
        }
        // After routine registration
        crate::initialize();
    });
}

fn r_test_setup() {
    // Build the argument list for Rf_initialize_R
    let mut arguments = cargs!["R", "--slave", "--no-save", "--no-restore"];

    unsafe {
        Rf_initialize_R(
            arguments.len() as i32,
            arguments.as_mut_ptr() as *mut *mut c_char,
        );
        libr::set(R_CStackLimit, usize::MAX);
        setup_Rmainloop();
    }
}

#[cfg(test)]
mod tests {
    use crate::r_task;

    #[test]
    fn test_stack_info() {
        // These tests assert that we've correctly turned off the `R_StackLimit` check during harp
        // and ark tests that use `r_task()`. It is turned off in `r_test_setup()` above.
        r_task(|| {
            let size = harp::parse_eval_base("Cstack_info()[['size']]").unwrap();
            assert_eq!(harp::r_int_get(size.sexp, 0), harp::object::r_int_na());

            let current = harp::parse_eval_base("Cstack_info()[['current']]").unwrap();
            assert_eq!(harp::r_int_get(current.sexp, 0), harp::object::r_int_na());
        })
    }
}
