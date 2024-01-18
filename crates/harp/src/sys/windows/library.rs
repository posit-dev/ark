/*
 * library.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use std::path::PathBuf;

use libloading::os::windows::Library;
use libloading::os::windows::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR;
use libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32;

pub fn open_r_shared_library(path: &PathBuf) -> Result<libloading::Library, libloading::Error> {
    // Each R shared library may have its own set of DLL dependencies. For example,
    // `R.dll` depends on `Rblas.dll` and some DLLs in system32. For each of the R DLLs we load,
    // the combination of R's DLL folder (i.e. `bin/x64`) and the system32 folder are enough to
    // load it, so we instruct libloading to tell the Windows function `LoadLibraryExW()` to
    // search those two places when looking for dependencies.
    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;

    let library = unsafe { Library::load_with_flags(path, flags) };

    // Map from the OS specific `Library` into the cross platform `Library`
    let library = library.map(|library| library.into());

    library
}
