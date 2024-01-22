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

use crate::library::find_r_shared_library;
use crate::library::open_and_leak_r_shared_library;

pub struct RLibraries {
    r: &'static libloading::Library,
    r_graphapp: &'static libloading::Library,
    r_lapack: &'static libloading::Library,
    r_iconv: &'static libloading::Library,
    r_blas: &'static libloading::Library,
}

impl RLibraries {
    pub fn from_r_home_path(path: &PathBuf) -> Self {
        let r_path = find_r_shared_library(&path, "R");
        let r = open_and_leak_r_shared_library(&r_path);

        // On Windows, we preemptively open the supporting R DLLs that live in
        // `bin/x64/` before starting R. R packages are allowed to link to these
        // DLLs, like stats, and they must be able to find them when the packages
        // are loaded. Because we don't add the `bin/x64` folder to the `PATH`,
        // we instead open these 4 DLLs preemptively and rely on the fact that the
        // "Loaded-module list" is part of the standard search path for dynamic link
        // library searching.
        // https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order
        let r_graphapp_path = find_r_shared_library(&path, "Rgraphapp");
        let r_graphapp = open_and_leak_r_shared_library(&r_graphapp_path);

        let r_lapack_path = find_r_shared_library(&path, "Rlapack");
        let r_lapack = open_and_leak_r_shared_library(&r_lapack_path);

        let r_iconv_path = find_r_shared_library(&path, "Riconv");
        let r_iconv = open_and_leak_r_shared_library(&r_iconv_path);

        let r_blas_path = find_r_shared_library(&path, "Rblas");
        let r_blas = open_and_leak_r_shared_library(&r_blas_path);

        Self {
            r,
            r_graphapp,
            r_lapack,
            r_iconv,
            r_blas,
        }
    }

    pub fn initialize_pre_setup_r(&self) {
        // R
        libr::initialize::functions(self.r);
        libr::initialize::functions_variadic(self.r);
        libr::initialize::mutable_globals(self.r);

        // Rgraphapp
        libr::graphapp::initialize::functions(self.r_graphapp);
    }

    pub fn initialize_post_setup_r(&self) {
        libr::initialize::constant_globals(self.r);
    }
}

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

pub fn find_r_shared_library_folder(path: &PathBuf) -> PathBuf {
    path.join("bin").join("x64")
}
