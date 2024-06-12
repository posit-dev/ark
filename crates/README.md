This repository contains five individual projects, which are evolving together.

- **Amalthea**, a Rust framework for building Jupyter and Positron kernels.
- **ARK**, the Amalthea R Kernel. ARK is a native kernel for R built on the Amalthea framework that interacts with the R interpreter in the same way RStudio does (it's a real frontend). It also implements the Language Server Protocol, using [tower-lsp](https://github.com/ebkalderon/tower-lsp).
- **echo**, a toy kernel for a fictional language that can be used to experiment with the kernel framework without the nuisance of getting language bindings working. As the name implies, it is a language that just echoes its input back as output.
- **harp**, safe Rust wrappers for R objects and interfaces.
- **stdext**, extensions to Rust's standard library for utility use in the other four projects.
