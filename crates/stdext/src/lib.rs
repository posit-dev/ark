//
// lib.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

pub mod all;
pub mod any;
pub mod case;
pub mod event;
pub mod join;
pub mod local;
pub mod ok;
pub mod push;
pub mod result;
pub mod spawn;
pub mod testing;
pub mod unwrap;

pub use crate::join::Joined;
pub use crate::ok::Ok;
pub use crate::push::Push;
pub use crate::testing::IS_TESTING;
pub use crate::unwrap::IntoOption;
pub use crate::unwrap::IntoResult;

/// Asserts that the given expression matches the given pattern
/// and optionally some further assertions.
///
/// To use until `assert_matches()` stabilises
///
/// # Examples
///
/// ```
/// #[macro_use] extern crate stdext;
/// # fn main() {
/// assert_match!(1 + 1, 2);
/// assert_match!(1 + 1, 2 => {
///    assert_eq!(40 + 2, 42)
/// });
/// # }
/// ```
#[macro_export]
macro_rules! assert_match {
    ($expression:expr, $pattern:pat_param => $code:block) => {
        match $expression {
            $pattern => $code,
            _ => panic!("Expected {}", stringify!($pattern)),
        }
    };

    ($expression:expr, $pattern:pat_param) => {
        assert!(matches!($expression, $pattern))
    };
}

// Useful for debugging
pub fn log_trace() {
    log::error!("{}", std::backtrace::Backtrace::force_capture().to_string());
}
