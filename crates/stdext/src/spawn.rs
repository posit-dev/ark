//
// spawn.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

#[macro_export]
macro_rules! spawn {
    ($name:expr, $body:expr) => {
        std::thread::Builder::new()
            .name($name.to_string())
            .spawn($body)
            .unwrap()
    };
    ($scope:ident, $name:expr, $body:expr) => {
        $scope
            .builder()
            .name($name.to_string())
            .spawn($body)
            .unwrap()
    };
}

/// Rust's own default. For a thread whose call tree reaches into dependencies
/// too deep to bound by inspection.
pub const DEFAULT_STACK_SIZE: usize = 2 * 1024 * 1024;

/// For a thread that walks data structures and does I/O, with no recursion over
/// user input.
pub const SMALL_STACK_SIZE: usize = 512 * 1024;

/// For a poll or dispatch loop whose deepest frame is a fixed-size buffer.
pub const TINY_STACK_SIZE: usize = 256 * 1024;

/// Like [`spawn!`], with an explicit stack size instead of
/// [`DEFAULT_STACK_SIZE`].
///
/// Budget for more than the thread call tree: a panic hook that captures a
/// backtrace runs on the panicking thread's stack. We don't go below 256kb to
/// remain on the safe side.
#[macro_export]
macro_rules! spawn_with_stack_size {
    ($name:expr, $stack_size:expr, $body:expr) => {
        std::thread::Builder::new()
            .name($name.to_string())
            .stack_size($stack_size)
            .spawn($body)
            .unwrap()
    };
}
