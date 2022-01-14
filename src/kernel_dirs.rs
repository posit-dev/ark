/*
 * kernel_dirs.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use std::path::Path;
use std::path::PathBuf;
use std::{env, fs};

/// Returns the path where Jupyter kernels should be/are installed.
pub fn jupyter_kernel_path() -> Result<PathBuf> {}

/// Returns the root Jupyter directory; uses the `JUPYTER_PATH` environment
/// variable if set, XDG values if not.
fn jupyter_dir() -> Option<PathBuf> {
    if let Ok(envpath) = env::var("JUPYTER_PATH") {
        Some(PathBuf::from(envpath))
    } else if let Some(userpath) = jupyter_xdg_dir() {
        Some(userpath)
    } else {
        None
    }
}

// Returns the XDG root directory for Jupyter
fn jupyter_xdg_dir() -> Option<PathBuf> {}

#[cfg(not(target_os = "macos"))]
fn jupyter_xdg_dir() -> Option<PathBuf> {}

#[cfg(target_os = "macos")]
fn jupyter_xdg_dir() -> Option<PathBuf> {}
