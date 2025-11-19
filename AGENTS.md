# AGENTS.md

This file provides guidance LLM agents and contributors when working with code in this repository.

## Project Overview

Ark is an R kernel for Jupyter applications, primarily created to serve as the interface between R and the Positron IDE. It is compatible with all frontends implementing the Jupyter protocol.

The project includes:
- A Jupyter kernel for structured interaction between R and a frontend
- An LSP server for intellisense features (completions, jump-to-definition, diagnostics)
- A DAP server for step-debugging of R functions

## Repository Structure

The codebase is organized as a Rust workspace containing multiple crates:

- **ark**: The main R Kernel implementation
- **harp**: Rust wrappers for R objects and interfaces
- **libr**: Bindings to R (dynamically loaded using `dlopen`/`LoadLibrary`)
- **amalthea**: A Rust framework for building Jupyter and Positron kernels
- **echo**: A toy kernel for testing the kernel framework
- **stdext**: Extensions to Rust's standard library used by the other projects

## Common Development Commands

### Building the Project

```bash
# Build the entire project
cargo build

# Build in release mode
cargo build --release

# On Windows: If Positron is running with a debug build of ark,
# Windows file locking prevents overwriting ark.exe. For interim "progress"
# checks during development, just check the specific crate you're working on
# instead:
cargo check --package ark
# You'll have to quit Positron to do `cargo build`, though, on Windows.
```

### Running Tests

We use `nextest` to run tests, which we invoke through our `just` runner. `just test` expands to `cargo nextest run`. Arguments are forwarded and nextest generally supports the same arguments as `cargo test`.

```bash
# Run all tests
just test

# Run specific tests
# Much better output than `cargo test <testname>`
just test <test_name>

# Run tests for a specific crate
just test -p ark
```

### Required R Packages for Testing

The following R packages are required for tests:
- data.table
- dplyr
- rstudioapi
- tibble
- haven
- R6

### Installation

After building, you can install the Jupyter kernel specification with:

```bash
./target/debug/ark --install
# or in release mode
./target/release/ark --install
```

## Generated code

Some of the files below `crates/amalthea/src/comm/` are automatically generated from comms specified in the Positron front end.
Such files always have `// @generated` at the top and SHOULD NEVER be edited "by hand".
If changes are needed in these files, that must happen in the separate Positron source repository and the comms for R and Python must be regenerated.

## Coding Style

- Do not use `bail!`. Instead use an explicit `return Err(anyhow!(...))`.

- When a `match` expression is the last expression in a function, omit `return` keywords in match arms. Let the expression evaluate to the function's return value.

- For error messages and logging, prefer direct formatting syntax: `Err(anyhow!("Message: {err}"))` instead of `Err(anyhow!("Message: {}", err))`. This also applies to `log::error!` and `log::warn!` and `log::info!` macros.

- Use `log::trace!` instead of `log::debug!`.

- Use fully qualified result types (`anyhow::Result`) instead of importing them.

- You can log `Result::Err` by using the `.log_err()` method from the extension trait `stdext::ResultExt`. Add some `.context()` if that would be helpful, but never do it for errors that are quite unexpected, such as from `.send()` to a channel (that would be too verbose).

- When writing tests, prefer simple assertion macros without custom error messages:
    - Use `assert_eq!(actual, expected);` instead of `assert_eq!(actual, expected, "custom message");`
    - Use `assert!(condition);` instead of `assert!(condition, "custom message");`

- When you extract code in a function (or move things around) that function goes _below_ the calling function. A general goal is to be able to read linearly from top to bottom with the relevant context and main logic first. The code should be organised like a call stack. Of course that's not always possible, use best judgement to produce the clearest code organization.

- Keep the main logic as unnested as possible. Favour Rust's `let ... else` syntax to return early or continue a loop in the `else` clause, over `if let`.

- Always prefer importing with `use` instead of qualifying with `::`, unless specifically requested in these instructions or by the user, or you see existing `::` usages in the file you're editing.
