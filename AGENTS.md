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

### Kernel and DAP Test Infrastructure

Integration tests for the kernel and DAP server live in `crates/ark/tests/` and use the test utilities from `crates/ark_test/`.

**Key components:**

- **`DummyArkFrontend`**: A mock Jupyter frontend that communicates with the kernel over ZMQ sockets. Use `DummyArkFrontend::lock()` to acquire it (only one per process).

- **`DapClient`**: A DAP client for testing the debugger. Obtained via `frontend.dap_client()` after starting the kernel.

**Stream handling:**

All `recv_iopub_*` methods automatically skip and buffer stream messages. This means:
- Non-stream assertions read cleanly without worrying about interleaved streams
- Stream content is accumulated in internal buffers
- Use `assert_stream_stdout_contains()` or `assert_stream_stderr_contains()` to check stream content
- **Stream assertions must be placed BEFORE `recv_iopub_idle()`** within each busy/idle window
- `recv_iopub_idle()` acts as a synchronization point that flushes stream buffers and panics if streams were received but not asserted

**Common patterns:**

```rust
// Lock the frontend and send an execute request
let frontend = DummyArkFrontend::lock();
frontend.send_execute_request("1 + 1", ExecuteRequestOptions::default());
frontend.recv_iopub_busy();
frontend.recv_iopub_execute_input();
frontend.recv_iopub_execute_result();
frontend.recv_iopub_idle();
frontend.recv_shell_execute_reply();

// For debug flows with streams, stream assertions MUST come before idle
frontend.recv_iopub_start_debug();
frontend.recv_iopub_stop_debug();
frontend.recv_iopub_start_debug();
frontend.assert_stream_stdout_contains("Called from:");
frontend.assert_stream_stdout_contains("debug at");
frontend.recv_iopub_idle();  // Flushes stream buffers, resets for next operation
frontend.recv_shell_execute_reply();

// For ordering assertions, use drain_streams() at checkpoints
frontend.recv_iopub_stop_debug();
let after_stop = frontend.drain_streams();
frontend.recv_iopub_start_debug();
assert!(after_stop.stdout.contains("debugging in:"));
```

**Debugging tests:**

Log messages (from the `log` crate) are not shown in test output. Use `eprintln!` for printf-style debugging.

Enable message tracing to see timestamped DAP and IOPub message flows:

```bash
ARK_TEST_TRACE=1 just test test_name      # All messages
ARK_TEST_TRACE=dap just test test_name    # DAP events only
ARK_TEST_TRACE=iopub just test test_name  # IOPub messages only
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

- For error messages and logging, prefer direct formatting syntax: `Err(anyhow!("Message: {err}"))` instead of `Err(anyhow!("Message: {}", err))`. This also applies to `log::error!` and `log::warn!` and `log::info!` macros. For logging errors specifically, use Debug formatting `{err:?}` to get more detailed error information.

- Use `log::trace!` instead of `log::debug!`.

- Use fully qualified result types (`anyhow::Result`) instead of importing them.

- You can log `Result::Err` by using the `.log_err()` method from the extension trait `stdext::ResultExt`. Add some `.context()` if that would be helpful, but never do it for errors that are quite unexpected, such as from `.send()` to a channel (that would be too verbose).

- Avoid `.unwrap()` and `.expect()`. For truly unrecoverable errors, use an explicit match with a `panic!` branch. For recoverable errors, use `.log_err()` or propagate with `?`.

- Avoid unnecessary `.clone()`. Prefer returning `&str` over `String` from accessors so callers only allocate when they need to. Reorder operations to avoid cloning (e.g., build a response before consuming the source data). For `Arc`, use `Arc::clone(&x)` instead of `x.clone()` to make the cheap clone obvious.

- Keep `Cargo.toml` dependencies in alphabetical order.

- When writing tests, prefer simple assertion macros without custom error messages:
    - Use `assert_eq!(actual, expected);` instead of `assert_eq!(actual, expected, "custom message");`
    - Use `assert!(condition);` instead of `assert!(condition, "custom message");`

- In tests, prefer exact assertions over fuzzy ones. Use `assert_eq!(stack[0].name, "foo()")` rather than `assert!(names.contains(&"foo()"))` when ordering and completeness matter.

- When you extract code in a function (or move things around) that function goes _below_ the calling function. A general goal is to be able to read linearly from top to bottom with the relevant context and main logic first. The code should be organised like a call stack. Of course that's not always possible, use best judgement to produce the clearest code organization.

- Keep the main logic as unnested as possible. Favour Rust's `let ... else` syntax to return early or continue a loop in the `else` clause, over `if let`.

- Always prefer importing with `use` instead of qualifying with `::`, unless specifically requested in these instructions or by the user, or you see existing `::` usages in the file you're editing.

- When two code paths do analogous things, make them structurally parallel so the symmetry is visible.

- Don't let comments drift from the code. If behaviour changes, update nearby comments. If a file is renamed, update its header comment.

- Use the new async closure syntax, e.g. `async move || { ... }` instead of `|| async move { ... }`.
