This repository contains several individual projects, which are evolving together.

- **ark**, the Ark R Kernel. Ark is a native kernel for R built on the Amalthea framework that interacts with the R interpreter in the same way RStudio does (i.e. it's implemented on top of the frontend API of R). It also implements the Language Server Protocol, using [tower-lsp](https://github.com/ebkalderon/tower-lsp).

- **harp**, Rust wrappers for R objects and interfaces. This is intended as an internal utility library for Ark rather than a general purpose API for R like [extendr](https://github.com/extendr/extendr).

- **libr**, our bindings to R. These are designed to be dynamically loaded using `dlopen()` on Unixes or `LoadLibrary()` on Windows. This is in contrast to [libR-sys](https://github.com/extendr/libR-sys) bindings which are meant to be used with R linked at build-time.

- **amalthea**, a Rust framework for building Jupyter and Positron kernels.

- **echo**, a toy kernel for a fictional language that can be used to experiment with the kernel framework without the nuisance of getting language bindings working. As the name implies, it is a language that just echoes its input back as output.

- **stdext**, extensions to Rust's standard library for utility use in the other internal projects.
