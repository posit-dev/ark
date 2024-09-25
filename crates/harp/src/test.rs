//
// test.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

// Helper functions for ensuring R is running before running tests
// that rely on an R session being available.

// TODO: Rust isn't smart enough to see that these methods are used in tests?
// We explicitly disable the warnings here since 'start_r()' is used by tests
// in other files.
#![allow(dead_code)]

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

// Escape hatch for unit tests. We need this because the default
// implementation of `r_task()` needs a fully formed `RMain` to send the
// task to, which we don't have in unit tests. Consequently tasks run
// immediately in the current thread in unit tests. Since each test has its
// own thread, they are synchronised via the `R_RUNTIME_LOCK` mutex.
pub static mut R_TASK_BYPASS: bool = false;

// This needs to be a reentrant mutex because many of our tests are wrapped in
// `r_test()` which takes the R lock. Without a reentrant mutex, we'd get
// deadlocked when we cause some other background thread to use an `r_task()`.
pub static mut R_RUNTIME_LOCK: parking_lot::ReentrantMutex<()> =
    parking_lot::ReentrantMutex::new(());

// This global variable is a workaround to enable test-only features or
// behaviour in integration tests (i.e. tests that live in `crate/tests/` as
// opposed to tests living in `crate/src/`).
//
// - Unfortunately we can't use `cfg(test)` in integration tests because they
//   are treated as an external crate.
//
// - Unfortunately we cannot move some of our integration tests to `src/`
//   because they must be run in their own process (e.g. because they are
//   running R).
//
// - Unfortunately we can't use the workaround described in
//   https://github.com/rust-lang/cargo/issues/2911#issuecomment-749580481
//   to enable a test-only feature in a self dependency in the dev-deps section
//   of the manifest file because Rust-Analyzer doesn't support such
//   circular dependencies: https://github.com/rust-lang/rust-analyzer/issues/14167.
//   So instead we use the same trick with harp rather than ark, so that there
//   is no circular dependency, which fixes the issue with Rust-Analyzer.
//
// - Unfortunately we can't query the features enabled in a dependency with `cfg`.
//   So instead we define a global variable here that can then be checked at
//   runtime in Ark.
pub static IS_TESTING: bool = cfg!(feature = "testing");

static INIT: Once = Once::new();

pub fn r_test<F: FnOnce()>(f: F) {
    let guard = unsafe { R_RUNTIME_LOCK.lock() };

    r_test_init();
    f();

    drop(guard);
}

pub fn r_test_init() {
    INIT.call_once(|| {
        unsafe {
            R_TASK_BYPASS = true;
            R_MAIN_THREAD_ID = Some(std::thread::current().id());
        }

        // Set up R_HOME if necessary.
        let r_home = match std::env::var("R_HOME") {
            Ok(r_home) => PathBuf::from(r_home),
            Err(_) => {
                let result = Command::new("R").arg("RHOME").output().unwrap();
                let r_home = String::from_utf8(result.stdout).unwrap();
                let r_home = r_home.trim();
                std::env::set_var("R_HOME", r_home);
                PathBuf::from(r_home)
            },
        };

        let libraries = RLibraries::from_r_home_path(&r_home);
        libraries.initialize_pre_setup_r();

        setup_r();

        libraries.initialize_post_setup_r();

        // Initialize harp globals
        unsafe {
            crate::routines::r_register_routines();
        }
        // After routine registration
        crate::initialize();
    });
}

fn setup_r() {
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
