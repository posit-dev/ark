/*
 * kernel_spec.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

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

    }
}

 