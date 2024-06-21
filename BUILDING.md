## Building

Install Rust. If you don't already have it, use `rustup`, following the [installation instructions at rustup.rs](https://rustup.rs/). In brief:

```bash
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
$ source $HOME/.cargo/env
```

Assuming you have a working Rust toolchain, you can just run `cargo build` at the top-level of this repository:

```sh
$ cargo build
```


## Standalone usage

To use ARK as a standalone kernel (outside Positron), install the kernelspec. From the repository root after running `cargo build`:

```sh
$ ./target/debug/ark --install
```

This installs a JSON file to the Jupyter kernel registry. After it completes, the Amalthea R kernel (ARK) will be available on all Jupyter frontends on your system (Notebook, Lab, etc.).

You will usually want to tweak the **ark** environment for development; add this to `~/Library/Jupyter/kernels/ark/kernel.json`:

```json
  "env": {
    "RUST_BACKTRACE": "1",
    "RUST_LOG": "warn,ark=trace",
  }
```

This enables backtrace capturing in [anyhow](https://docs.rs/anyhow) errors and sets internal crates to log at TRACE level and external dependencies to log at WARN. Setting the latter to more verbose levels can dramatically decrease performance. See the documentation in the [tracing_subscriber](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) crate for more fine-grained tuning of the `RUST_LOG` environment variable.
