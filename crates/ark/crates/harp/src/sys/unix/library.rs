/*
 * library.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use std::path::PathBuf;
use std::process::Command;

use anyhow::anyhow;
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
        // Before we open the libraries, set `DYLD_FALLBACK_LIBRARY_PATH` or
        // `LD_LIBRARY_PATH` as needed
        set_library_path_env_var(path);

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

#[cfg(target_os = "macos")]
const LIBRARY_PATH_ENVVAR: &'static str = "DYLD_FALLBACK_LIBRARY_PATH";
#[cfg(target_os = "linux")]
const LIBRARY_PATH_ENVVAR: &'static str = "LD_LIBRARY_PATH";

fn set_library_path_env_var(path: &PathBuf) {
    // In the future, we may add additional paths to the env var beyond just what R
    // gives us, like RStudio does.
    // https://github.com/rstudio/rstudio/blob/50d1a034a04188b42cf7560a86a268a95e62d129/src/cpp/core/r_util/REnvironmentPosix.cpp#L817

    let mut paths = Vec::new();

    // Expect that this includes the existing env var value, if there was one
    match source_ldpaths_script(path) {
        Ok(path) => paths.push(path),
        Err(err) => log::error!("Failed to source `ldpaths` script: {err:?}."),
    }

    // Only set if we have something
    if paths.is_empty() {
        return;
    }

    let paths = paths.join(":");

    log::info!("Setting '{LIBRARY_PATH_ENVVAR}' env var to '{paths}'.");

    std::env::set_var(LIBRARY_PATH_ENVVAR, paths);
}

/// Source `{R_HOME}/etc/ldpaths`
///
/// - On macOS, this is for `DYLD_FALLBACK_LIBRARY_PATH`
/// - On linux, this is for `LD_LIBRARY_PATH`
///
/// This is a file that R provides which adds the `{R_HOME}/lib/` directory and a Java
/// related directory (relevant for rJava, apparently) to the relevant library path env
/// var.
///
/// Adding R's `lib/` directory to the front of `LD_LIBRARY_PATH` is particularly
/// important. We open `libR` with `RTLD_GLOBAL`, but there are other libs shipped by R
/// in that `lib/` folder that other packages might link to, and having the `lib/` folder
/// included in `LD_LIBRARY_PATH` is how those packages will find those libs.
fn source_ldpaths_script(path: &PathBuf) -> anyhow::Result<String> {
    let ldpaths = path.join("etc").join("ldpaths");

    let Some(ldpaths) = ldpaths.to_str() else {
        let ldpaths = ldpaths.to_string_lossy();
        return Err(anyhow!(
            "Failed to convert `ldpaths` path to UTF-8 string: '{ldpaths}'"
        ));
    };

    // Source (i.e. `.`) the `ldpaths` file into the current bash session, and then
    // print out the relevant env var that it set. `printf` is more portable than `echo -n`.
    let command = format!(". {ldpaths} && printf '%s' \"${LIBRARY_PATH_ENVVAR}\"");

    // Need to ensure `R_HOME` is set, as `ldpaths` references it.
    // Expect that `ldpaths` appends to an existing env var if there is one,
    // rather than overwriting it, so we don't have to do that.
    let output = Command::new("sh")
        .env("R_HOME", &path)
        .arg("-c")
        .arg(command)
        .output()?;

    if !output.status.success() {
        let status = output.status;
        return Err(anyhow!("Failed with status: {status}"));
    }
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8(output.stderr)?;
        return Err(anyhow!("Unexpected output on stderr: '{stderr}'"));
    }

    let value = String::from_utf8(output.stdout)?;

    if value.is_empty() {
        return Err(anyhow!(
            "Empty string returned for '{LIBRARY_PATH_ENVVAR}'. Expected at least one path."
        ));
    }

    Ok(value)
}
