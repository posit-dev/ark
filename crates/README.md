This repository contains five individual projects, which are evolving together.

- **Amalthea**, a Rust framework for building Jupyter and Positron kernels.

- **ARK**, the Amalthea R Kernel. ARK is a native kernel for R built on the Amalthea framework that interacts with the R interpreter in the same way RStudio does (i.e. it's implemented on top of the frontend API of R). It also implements the Language Server Protocol, using [tower-lsp](https://github.com/ebkalderon/tower-lsp).

- **echo**, a toy kernel for a fictional language that can be used to experiment with the kernel framework without the nuisance of getting language bindings working. As the name implies, it is a language that just echoes its input back as output.

- **harp**, Rust wrappers for R objects and interfaces. This is intended as an internal utility library for Ark rather than a general purpose API for R like [extendr](https://github.com/extendr/extendr).

- **stdext**, extensions to Rust's standard library for utility use in the other internal projects.
