/*
 * kernel_spec.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

 use crate::kernel_dirs;

/// From the Jupyter documentation for [Kernel Specs](https://jupyter-client.readthedocs.io/en/stable/kernels.html#kernel-specs).
pub struct KernelSpec {

   /// List of command line arguments to be used to start the kernel
   pub argv: Vec<String>;

   // The kernel name as it should be displayed in the UI
   pub display_name: String;

   // The kernel's language
   pub language: String;
}


impl KernelSpec {
    pub fn install(&self) -> Result<()> {
        if let Some(kernel_dir) = kernel_dirs::jupyter_kernel_path() {
            install_to(kernel_dir.join("amalthea"))
            Ok(())
        }
        Err(concat!("Could not locate Jupyter installation directory. "
            "Check XDG_DATA_PATH environment variables and set JUPYTER_PATH if necessary."))
    }

    fn install_to(&self, path: PathBuf) -> () {
        ()
    }
}

 