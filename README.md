ark <img src="logo.webp" align="right" height=160 width=160 />
=======================================================

R kernel for Jupyter and Positron, written in Rust.

TODO


## Usage

### Building

Install Rust. If you don't already have it, use `rustup`, following the [installation instructions at rustup.rs](https://rustup.rs/). In brief:

```bash
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
$ source $HOME/.cargo/env
```

Assuming you have a working Rust toolchain, you can just run `cargo build`:

```bash
$ cargo build
```

### Standalone

To use ARK as a standalone kernel (outside Positron), install the kernelspec. From the repository root:

```bash
$ ./target/debug/ark --install
```

This installs a JSON file to the Jupyter kernel registry. After it completes, the Amalthea R kernel (ARK) will be available on all Jupyter frontends on your system (Notebook, Lab, Positron, etc.).

You will usually want to tweak the **ark** environment for development; add this to `~/Library/Jupyter/kernels/ark/kernel.json`:

```json
  "env": {
    "RUST_LOG": "trace",
    "R_HOME": "/Library/Frameworks/R.framework/Resources"
  }
```

where `R_HOME` is the location of your R installation. If you're unsure where this is, run `R RHOME`
and it will be printed to the console.

More fine-grained control of logging is available for `RUST_LOG` as documented in [env_logger](https://docs.rs/env_logger/0.9.0/env_logger/#enabling-logging).


### In Positron

By default, the Amalthea kernel is included in Positron's `positron-r` extension, as a submodule; it
powers the R experience in Positron.


## Related Projects

[Positron](https://github.com/rstudio/positron), a next-generation data science IDE

[IRKernel](https://github.com/IRkernel/IRkernel), a kernel for R written primarily in R itself
