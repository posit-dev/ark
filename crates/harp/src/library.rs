//
// library.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::env::consts::DLL_PREFIX;
use std::env::consts::DLL_SUFFIX;
use std::env::consts::OS;
use std::path::PathBuf;

/// Open an R shared library located at the specified `path`
///
/// The returned `Library` MUST stay open for the entirety of the time that we call the
/// R API. Currently this is managed by the fact that `run_r()` never returns.
/// However, we could also `Box::leak()` it if we wanted to be explicit about this.
pub fn open_r_shared_library(path: &PathBuf) -> libloading::Library {
    let library = unsafe { libloading::Library::new(&path) };

    let library = match library {
        Ok(library) => library,
        Err(err) => panic!(
            "The R shared library at '{}' could not be opened: {}",
            path.display(),
            err,
        ),
    };

    log::info!(
        "Successfully opened R shared library at '{}'.",
        path.display()
    );

    library
}

/// Navigate to an R shared library from `R_HOME`
///
/// i.e. like `R` or `Rgraphapp`
///
/// This assumes that the shared library is in the "standard place" below `R_HOME`, which
/// may not always prove to be true. If this ever fails, we will need to revisit our
/// assumptions.
pub fn find_r_shared_library(home: &PathBuf, name: &str) -> PathBuf {
    let folder = find_r_shared_library_folder(home);

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

fn find_r_shared_library_folder(home: &PathBuf) -> PathBuf {
    match OS {
        "macos" => home.join("lib"),
        "windows" => home.join("bin").join("x64"),
        "linux" => home.join("lib"),
        _ => panic!("Can't find shared library folder. Unknown OS: '{}'.", OS),
    }
}
