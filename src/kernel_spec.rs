/*
 * kernel_spec.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::kernel_dirs;
use log::trace;
use serde::Serialize;
use std::error;
use std::fmt;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

/// From the Jupyter documentation for [Kernel Specs](https://jupyter-client.readthedocs.io/en/stable/kernels.html#kernel-specs).
#[derive(Serialize)]
pub struct KernelSpec {
    /// List of command line arguments to be used to start the kernel
    pub argv: Vec<String>,

    // The kernel name as it should be displayed in the UI
    pub display_name: String,

    // The kernel's language
    pub language: String,
}

#[derive(Debug)]
pub enum InstallError {
    NoInstallDir,
    CreateDirFailed(std::io::Error),
    JsonSerializeFailed(serde_json::Error),
    CreateSpecFailed(std::io::Error),
    WriteSpecFailed(std::io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            InstallError::NoInstallDir => {
                write!(f, "No Jupyter installation directory found.")
            }
            InstallError::CreateDirFailed(err) => {
                write!(f, "Could not create directory: {}", err)
            }
            InstallError::JsonSerializeFailed(err) => {
                write!(f, "Could not serialize kernel spec to JSON: {}", err)
            }
            InstallError::CreateSpecFailed(err) => {
                write!(f, "Could not create kernel spec file: {}", err)
            }
            InstallError::WriteSpecFailed(err) => {
                write!(f, "Could not write kernel spec file: {}", err)
            }
        }
    }
}

impl error::Error for InstallError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            InstallError::NoInstallDir => None,
            InstallError::CreateDirFailed(ref e) => Some(e),
            InstallError::JsonSerializeFailed(ref e) => Some(e),
            InstallError::WriteSpecFailed(ref e) => Some(e),
            InstallError::CreateSpecFailed(ref e) => Some(e),
        }
    }
}

impl KernelSpec {
    pub fn install(&self, folder: String) -> Result<(), InstallError> {
        if let Some(kernel_dir) = kernel_dirs::jupyter_kernel_path() {
            return self.install_to(kernel_dir.join(folder));
        }
        return Err(InstallError::NoInstallDir);
    }

    fn install_to(&self, path: PathBuf) -> Result<(), InstallError> {
        // Ensure that the parent folder exists, and form a path to file we'll write
        if let Err(err) = fs::create_dir_all(&path) {
            return Err(InstallError::CreateDirFailed(err));
        }
        let dest = path.join("kernel.json");

        // Serialize the kernel spec to JSON
        match serde_json::to_string_pretty(self) {
            Ok(contents) => {
                trace!("Installing kernelspec JSON to {:?}: {}", dest, contents);
                match File::create(dest) {
                    Ok(mut f) => {
                        if let Err(err) = f.write_all(contents.as_bytes()) {
                            return Err(InstallError::WriteSpecFailed(err));
                        }
                    }
                    Err(err) => return Err(InstallError::CreateSpecFailed(err)),
                };
                Ok(())
            }
            Err(err) => {
                return Err(InstallError::JsonSerializeFailed(err));
            }
        }
    }
}
