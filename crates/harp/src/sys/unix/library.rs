/*
 * library.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use std::path::PathBuf;

use crate::library::find_r_shared_library;
use crate::library::open_and_leak_r_shared_library;

pub struct RLibraries {
    r: &'static libloading::Library,
}

impl RLibraries {
    pub fn from_r_home_path(path: &PathBuf) -> Self {
        let r_path = find_r_shared_library(&path, "R");
        let r = open_and_leak_r_shared_library(&r_path);

        Self { r }
    }

    /// Initialize dynamic bindings to functions and mutable globals. These are required
    /// to even start R (for things like `Rf_initialize_R()` and `R_running_as_main_program`).
    pub fn initialize_pre_setup_r(&self) {
        libr::initialize::functions(self.r);
        libr::initialize::functions_variadic(self.r);
        libr::initialize::mutable_globals(self.r);
    }

    /// After `setup_Rmainloop()` has run, which initializes R's "constant" global variables,
    /// we can initialize our own.
    pub fn initialize_post_setup_r(&self) {
        libr::initialize::constant_globals(self.r);
    }
}

pub fn open_r_shared_library(path: &PathBuf) -> Result<libloading::Library, libloading::Error> {
    unsafe { libloading::Library::new(&path) }
}

pub fn find_r_shared_library_folder(path: &PathBuf) -> PathBuf {
    path.join("lib")
}
