/*
 * kernel_dirs.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::env;
use std::path::PathBuf;

/// Returns the path where Jupyter kernels should be/are installed.
pub fn jupyter_kernel_path() -> Option<PathBuf> {
    if let Some(path) = jupyter_dir() {
        return Some(path.join("kernels"));
    }
    None
}

/// Returns the root Jupyter directory; uses the `JUPYTER_PATH` environment
/// variable if set, XDG values if not.
pub fn jupyter_dir() -> Option<PathBuf> {
    if let Ok(envpath) = env::var("JUPYTER_PATH") {
        Some(PathBuf::from(envpath))
    } else {
        jupyter_xdg_dir()
    }
}

/// Returns the XDG root directory for Jupyter
#[cfg(not(target_os = "macos"))]
fn jupyter_xdg_dir() -> Option<PathBuf> {
    // On Windows/Linux, the path is XDG_DATA_DIR/jupyter
    dirs::data_dir().map(|path| path.join("jupyter"))
}

#[cfg(target_os = "macos")]
fn jupyter_xdg_dir() -> Option<PathBuf> {
    // On MacOS, XDG_DATA_DIR is ~/Library/Application Support, but Jupyter
    // looks in ~/Library/Jupyter.
    dirs::data_dir().and_then(|path| path.parent().map(|parent| parent.join("Jupyter")))
}
