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
use std::sync::Mutex;
use std::sync::Once;

use libr::setup_Rmainloop;
use libr::R_CStackLimit;
use libr::Rf_initialize_R;
use stdext::cargs;

use crate::exec::r_sandbox;
use crate::library::RLibraries;
use crate::R_MAIN_THREAD_ID;

// Escape hatch for unit tests. We need this because the default
// implementation of `r_task()` needs a fully formed `RMain` to send the
// task to, which we don't have in unit tests. Consequently tasks run
// immediately in the current thread in unit tests. Since each test has its
// own thread, they are synchronised via the `R_RUNTIME_LOCK` mutex.
pub static mut R_TASK_BYPASS: bool = false;
static mut R_RUNTIME_LOCK: Mutex<()> = Mutex::new(());

static INIT: Once = Once::new();

pub fn start_r() {
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

pub fn r_test<F: FnOnce()>(f: F) {
    start_r();
    let guard = unsafe { R_RUNTIME_LOCK.lock() };

    if let Err(err) = r_sandbox(f) {
        panic!("While running test: {err:?}");
    }

    drop(guard);
}

#[macro_export]
macro_rules! r_test {
    ($($expr:tt)*) => {
        #[allow(unused_unsafe)]
        $crate::test::r_test(|| unsafe { $($expr)* })
    }
}
