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
    /// We are about to dynamically load libR into the Ark process using `dlopen()` on
    /// Unix / `LibraryLoad()` on Windows. This is a bit unusual as ordinarily frontends
    /// link to R at launch-time. The goal is to get full control of symbol access, making
    /// it easy to provide compatibility implementations on older versions of R. However
    /// this does require some precautions on Unixes to ensure the behaviour is as close
    /// as possible to launch-time linking.
    ///
    /// - We set the `RTLD_GLOBAL` option to expose all libR symbols to subsequently
    ///   loaded plugins. This is similar to how linking to a library at launch time
    ///   exposes symbols globally, including to loaded plugins.
    ///
    /// - Despite being loaded with global scope, libR is not considered opened by the
    ///   dynamic loader. This is problematic when loading package libraries because they
    ///   typically link against libR. Even though we have exposed all the symbols they
    ///   need, and opening a libR file would normally won't have any further effect (it
    ///   could in special cases involving version mismatches), the linker will fail to
    ///   load the package library if it can't find a libR file.
    ///
    ///   To work correctly with the variety of ways package libraries are linked against
    ///   R, with relative (common on Linux) or absolute (common on macOS) paths, Ark
    ///   should be launched in an environment where `LD_LIBRARY_PATH` (Linux) and
    ///   `DYLD_LIBRARY_PATH` / `DYLD_FALLBACK_LIBRARY_PATH` (macOS) point to the `lib`
    ///   folder of the target `R_HOME`. This will allow package libraries to link against
    ///   a libR library. This library will never be used in practice as the symbols
    ///   exposed via `RTLD_GLOBAL` will have precedence.
    ///
    ///   In the edge case where a package is compiled against a newer version of R and
    ///   linked with an absolute path, having opened the older R first will prevent the
    ///   newer R from being loaded, and the newer symbols from being resolved into that
    ///   different library. In this case users get undefined symbols errors on load
    ///   instead of undefined behaviour and crashes.
    ///
    ///   Alternatively we could link to an empty libR shipped with Ark. Linking to the
    ///   real one is more convenient and also takes care of other libraries in there such
    ///   as libRblas.
    ///
    /// - On macOS we really want to add `{R_HOME}/lib` to `DYLD_LIBRARY_PATH` and not
    ///   `DYLD_FALLBACK_LIBRARY_PATH`. The former ensures our libR is always selected.
    ///   The latter would allow a package linked with an absolute path to a different
    ///   version of R to open that different libR, causing potential UB instead of
    ///   undefined symbol errors.
    ///
    /// - On macOS, your build of Ark needs the `allow-dyld-environment-variables`
    ///   entitlement to allow the Ark process to inherit the `DYLD_LIBRARY_PATH`
    ///   environment variable.
    ///
    /// - In addition to `{R_HOME}/lib`, it's also useful for the caller of Ark to include
    ///   `R_JAVA_LD_LIBRARY_PATH` in the load list. This time on macOS it makes sense to
    ///   use `DYLD_FALLBACK_LIBRARY_PATH`, if only to be consistent with
    ///   `{R_HOME}/etc/ldpaths`, where this envvar is normally defined. Note that this
    ///   might cause a package linked to Java with an absolute path to decide for all
    ///   subsequently loaded packages which version of Java Ark is linked with.
    ///
    /// - Windows doesn't need these precautions because symbol lookup is namespaced to
    ///   the library. On Unix, symbol lookup is global and resolved via a global linked
    ///   list of library namespaces.
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
