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

This installs a JSON file to the Jupyter kernel registry. After it completes, the Ark R kernel will be available on all Jupyter frontends on your system (Notebook, Lab, etc.).

You will usually want to tweak the **ark** environment for development; add this to `~/Library/Jupyter/kernels/ark/kernel.json`:

```json
  "env": {
    "RUST_BACKTRACE": "1",
    "RUST_LOG": "warn,ark=trace",
  }
```

This enables backtrace capturing in [anyhow](https://docs.rs/anyhow) errors and sets internal crates to log at TRACE level and external dependencies to log at WARN. Setting the latter to more verbose levels can dramatically decrease performance. See the documentation in the [tracing_subscriber](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) crate for more fine-grained tuning of the `RUST_LOG` environment variable.

## Test with Release Positron

To test the dev build of ARK on Release Positron, you can open Positron's user settings
and change option `Positron > R > Kernel: Path` (ID: `positron.r.kernel.path`)
to the location of the binary.

```json
{
    "positron.r.kernel.path": "/path/to/ark/target/debug/ark",
}
```

With a development version of Positron, a development version of Ark is automatically detected as long as the `positron/` and `ark/` folders are at the same depth, i.e.:

```
| files/
| |- ark/
| |- positron/
```

## Testing

We use [nextest](https://nexte.st/) for testing rather than a standard `cargo test`, primarily because nextest runs each test in its own process rather than in its own thread.
This is critical for us, as Ark has global objects that can only be set up once per process (such as setup around the R process itself).
Additionally, using one process per test means that it is impossible for one test to interfere with another (so you don't have to worry about test cleanup, particularly if you add objects to R's global environment).
Tests are still run in parallel, using multiple processes, and this ends up being quite fast and reliable.

Install the nextest cli tool using a [prebuilt binary](https://nexte.st/docs/installation/pre-built-binaries/).

Run tests locally with `just test` (which runs `cargo nextest run`) or `Tasks: Run Test Task` in VS Code (which you can bind to a keyboard shortcut).
Run insta snapshot tests in "update" mode with `just test-insta` (which runs `cargo insta test --test-runner nextest`).

On CI we use the nextest profile found in `.config/nextest.toml`.

## Just

We use [just](https://github.com/casey/just) as a simple command runner, and the shortcuts live in `justfile`.
On macOS, install just with `brew install just`.
On Windows, we've used `cargo install just` with success.
For more installation tips, see the just README.
