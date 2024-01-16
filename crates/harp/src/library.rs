//
// library.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

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
            "The `R` shared library at '{}' could not be opened: {}",
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

/// Navigate to the R shared library from `R_HOME`
///
/// * On macOS: `libR.dylib`
/// * On Windows: `R.dll`
/// * On Linux: `libR.so`
///
/// This assumes that the shared library is in the "standard place" below `R_HOME`, which
/// may not always prove to be true. If this ever fails, we will need to revisit our
/// assumptions.
pub fn find_r_shared_library(home: &PathBuf) -> PathBuf {
    match std::env::consts::OS {
        "macos" => home.join("lib").join("libR.dylib"),
        "windows" => home.join("bin").join("x64").join("R.dll"),
        "linux" => home.join("lib").join("libR.so"),
        _ => panic!(
            "Can't find R shared library. Unknown OS: '{}'.",
            std::env::consts::OS
        ),
    }
}
