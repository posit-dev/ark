//
// library.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::env::consts::DLL_PREFIX;
use std::env::consts::DLL_SUFFIX;
use std::path::PathBuf;

use crate::sys;
pub use crate::sys::library::RLibraries;

/// Open an R shared library located at the specified `path`.
/// Returned with `'static` lifetime because we `Box::leak()` the `Library`.
pub(crate) fn open_and_leak_r_shared_library(path: &PathBuf) -> &'static libloading::Library {
    // Call system specific open helper
    let library = sys::library::open_r_shared_library(path);

    let library = match library {
        Ok(library) => library,
        Err(err) => panic!(
            "The R shared library at '{}' could not be opened: {:?}",
            path.display(),
            err,
        ),
    };

    log::info!(
        "Successfully opened R shared library at '{}'.",
        path.display()
    );

    // Leak the `Library` to ensure that it lives for the lifetime of the program (ark).
    // Otherwise, if the library closes then we can't safely access the functions inside it.
    let library = Box::new(library);
    let library = Box::leak(library);

    library
}

/// Navigate to an R shared library from `R_HOME`
///
/// i.e. like `R` or `Rgraphapp`
///
/// This assumes that the shared library is in the "standard place" below `R_HOME`, which
/// may not always prove to be true. If this ever fails, we will need to revisit our
/// assumptions.
pub(crate) fn find_r_shared_library(home: &PathBuf, name: &str) -> PathBuf {
    // Navigate to system specific library folder from `R_HOME`
    let folder = crate::sys::library::find_r_shared_library_folder(home);

    // i.e.
    // * On macOS: `libR.dylib`
    // * On Windows: `R.dll`
    // * On Linux: `libR.so`
    let name = DLL_PREFIX.to_string() + name + DLL_SUFFIX;

    let path = folder.join(name.as_str());

    match path.try_exists() {
        Ok(true) => return path,
        Ok(false) => panic!("Can't find R shared library '{}' at '{}'. If this is a custom build of R, ensure it is compiled with `--enable-R-shlib`.", name, path.display()),
        Err(err) => panic!("Can't determine if R shared library path exists: {err:?}"),
    }
}
