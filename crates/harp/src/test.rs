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
use libr::R_CStackLimit_set;
use libr::Rf_initialize_R;
use stdext::cargs;

use crate::exec::r_sandbox;
use crate::library;
use crate::library::find_r_shared_library;
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

        // TODO: Right now, tests can fail if the version of R discovered
        // on the PATH, and the version of R that 'ark' linked to at compile
        // time, do not match. We could relax this requirement by allowing
        // 'ark' to have undefined symbols, and use the DYLD_INSERT_LIBRARIES
        // trick to insert the right version of R when 'ark' is launched,
        // but for now we just have this comment as a reminder.

        // Set up R_HOME if necessary.
        let home = match std::env::var("R_HOME") {
            Ok(home) => PathBuf::from(home),
            Err(_) => {
                let result = Command::new("R").arg("RHOME").output().unwrap();
                let home = String::from_utf8(result.stdout).unwrap();
                let home = home.trim();
                std::env::set_var("R_HOME", home);
                PathBuf::from(home)
            },
        };

        // Find shared library from `R_HOME`
        let r_library_path = find_r_shared_library(&home, "R");
        let r_library = library::open_r_shared_library(&r_library_path);

        #[cfg(target_family = "windows")]
        let rgraphapp_library_path = library::find_r_shared_library(&home, "Rgraphapp");
        #[cfg(target_family = "windows")]
        let rgraphapp_library = library::open_r_shared_library(&rgraphapp_library_path);

        // Initialize functions and mutable globals so we can call the R setup functions
        libr::initialize::functions(&r_library);
        libr::initialize::functions_variadic(&r_library);
        libr::initialize::mutable_globals(&r_library);

        #[cfg(target_family = "windows")]
        libr::graphapp::initialize::functions(&rgraphapp_library);

        // Build the argument list for Rf_initialize_R
        let mut arguments = cargs!["R", "--slave", "--no-save", "--no-restore"];

        unsafe {
            Rf_initialize_R(
                arguments.len() as i32,
                arguments.as_mut_ptr() as *mut *mut c_char,
            );
            R_CStackLimit_set(usize::MAX);
            setup_Rmainloop();
        }

        // Now we can initialize constant globals since `setup_Rmainloop()` has run
        libr::initialize::constant_globals(&r_library);

        // Leak the library so it lives for the process lifetime, because unlike in our
        // normal setup, this function returns and will otherwise drop (and close) the library
        let r_library = Box::new(r_library);
        Box::leak(r_library);

        #[cfg(target_family = "windows")]
        let rgraphapp_library = Box::new(rgraphapp_library);
        #[cfg(target_family = "windows")]
        Box::leak(rgraphapp_library);

        // Initialize harp globals
        unsafe {
            crate::routines::r_register_routines();
        }
        crate::initialize();
    });
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
