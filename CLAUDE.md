# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

```bash
# Run all tests with nextest (recommended for CI)
cargo nextest run

# Run specific tests
cargo test <test_name>

# Run tests for a specific crate
cargo test -p ark
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

## Current Work in Progress

### Convert to Code Feature for Data Explorer

**Feature**: Backend implementation of "convert to code" for Positron's data explorer in R, allowing users to generate R code (dplyr syntax) that replicates their UI-based data manipulations (filters, sorting).

**Status**:
- ✅ R implementation has been created with awareness of the Python implementation
- ✅ Core feature is implemented and working with dplyr syntax
- ✅ Unit tests exist for string output validation
- ✅ An MVP exists of a test that validates the result of executing generated code

**Key files in R implementation**:
- `crates/ark/src/data_explorer/convert_to_code.rs` - Core conversion logic with traits and handlers + tests
- `crates/ark/src/data_explorer/r_data_explorer.rs` - Data explorer integration
- `crates/ark/tests/data_explorer.rs` - Integration tests for data explorer

**Key files in Python implementation** (for reference):
- `../positron/extensions/positron-python/python_files/posit/positron/convert.py` - Core conversion logic
- `../positron/extensions/positron-python/python_files/posit/positron/data_explorer.py` - Main data explorer (see `convert_to_code` methods around lines 1408, 2297)
- `../positron/extensions/positron-python/python_files/posit/positron/tests/test_convert.py` - Execution validation tests

**Architecture comparison**:
- Both R and Python use similar trait/abstract class patterns for extensibility
- R uses `PipeBuilder` for clean pipe chain generation; Python uses `MethodChainBuilder`
- Both have comprehensive filter/sort handlers with type-aware value formatting

**Possible next steps**:
1. Add more execution tests, e.g. for sorting, or combined filtering and sorting
1. Consider a "tidyverse" syntax instead of or in addition to "dplyr", where
   we would use stringr function for text search filters
1. Dig in to non-syntactic column names
1. Dig in to filtering for date and datetime columns
1. Handle "base" and "data.table" syntaxes
