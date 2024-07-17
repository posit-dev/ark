/*
 * library.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use std::path::PathBuf;

use libloading::os::unix::Library;
use libloading::os::unix::RTLD_GLOBAL;
use libloading::os::unix::RTLD_LAZY;

use crate::library::find_r_shared_library;
use crate::library::open_and_leak_r_shared_library;

pub struct RLibraries {
    r: &'static libloading::Library,
}

impl RLibraries {
    pub fn from_r_home_path(path: &PathBuf) -> Self {
        // On macOS and Linux, we rely on the fact that the parent process that
        // starts ark should have set `DYLD_FALLBACK_LIBRARY_PATH` or `LD_LIBRARY_PATH`
        // respectively already, referencing R's `{R_HOME}/etc/ldpaths` script to generate
        // the correct environment variable to set (which includes info about Java related
        // paths as well). Setting these env vars is critical, as they add `{R_HOME}/lib/`
        // to a place that `dlopen()` can find. Even though we open libR with
        // `RTLD_GLOBAL`, it seems that the path to libR (and other libraries in
        // `{R_HOME}/lib`) recorded in package info is often relative rather than absolute
        // on both Linux and macOS, and the env var ends up being the only way to reliably
        // locate libR when the package is being loaded.

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
    // Default behavior of `Library` is `RTLD_LAZY | RTLD_LOCAL`.
    // In general this makes sense, where you want to isolate modules as much as possible.
    // However, for us `libR` is like our main application.
    //
    // Setting `RTLD_GLOBAL` means that the symbols of the opened library (and its
    // dependencies) become available for subsequently loaded libraries WITHOUT them
    // needing to use `dlsym()`. Subsequent libraries here can correspond to R packages,
    // like `utils.so` or any R package with compiled code.
    //
    // The main reason we do this is that we believe this most closely reproduces what
    // happens when you link your application to `libR.so` at load time rather than
    // runtime (i.e. RStudio's `rsession` does load time linking). We believe load time
    // linking makes the libR library (and therefore its symbols) available globally to
    // downstream loaded libraries.
    //
    // More discussion in:
    // https://github.com/posit-dev/amalthea/pull/205
    let flags = RTLD_LAZY | RTLD_GLOBAL;

    let library = unsafe { Library::open(Some(&path), flags) };

    // Map from the OS specific `Library` into the cross platform `Library`
    let library = library.map(|library| library.into());

    library
}

pub fn find_r_shared_library_folder(path: &PathBuf) -> PathBuf {
    path.join("lib")
}
