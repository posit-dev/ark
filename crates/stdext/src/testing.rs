//
// testing.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

// This global variable is a workaround to enable test-only features or
// behaviour in integration tests (i.e. tests that live in `crate/tests/` as
// opposed to tests living in `crate/src/`).
//
// - Unfortunately we can't use `cfg(test)` in integration tests because they
//   are treated as an external crate.
//
// - Unfortunately we cannot move some of our integration tests to `src/`
//   because they must be run in their own process (e.g. because they are
//   running R).
//
// - Unfortunately we can't use the workaround described in
//   https://github.com/rust-lang/cargo/issues/2911#issuecomment-749580481
//   to enable a test-only feature in a self dependency in the dev-deps section
//   of the manifest file because Rust-Analyzer doesn't support such
//   circular dependencies: https://github.com/rust-lang/rust-analyzer/issues/14167.
//   So instead we use the same trick with stdext rather than ark, so that there
//   is no circular dependency, which fixes the issue with Rust-Analyzer.
//
// - Unfortunately we can't query the features enabled in a dependency with `cfg`.
//   So instead we define a global variable here that can then be checked at
//   runtime in Ark.
pub static IS_TESTING: bool = cfg!(feature = "testing");
