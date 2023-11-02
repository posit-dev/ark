//
// result.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::fmt::Display;

// Since this is a macro, this correctly records call site information.
// TODO: Should we retire the `ResultOrLog` trait?
#[macro_export(local_inner_macros)]
macro_rules! log_error {
    ($res:expr) => {
        if let Err(err) = $res {
            log::error!("{err}");
        }
    };
    ($prefix:expr, $res:expr) => {
        if let Err(err) = $res {
            log::error!("{}: {err}", $prefix);
        }
    };
}

#[macro_export(local_inner_macros)]
macro_rules! log_and_panic {
    ($arg:expr) => {
        log::error!($arg);
        log::error!("Backtrace:\n{}", std::backtrace::Backtrace::capture());
        log::logger().flush();
        std::panic!($arg);
    };
}

pub trait ResultOrLog<T, E> {
    /// If `self` is an error, log an error, else do nothing and consume self.
    fn or_log_error(self, prefix: &str);

    /// If `self` is an error, log a warning, else do nothing and consume self.
    fn or_log_warning(self, prefix: &str);

    /// If `self` is an error, log info, else do nothing and consume self.
    fn or_log_info(self, prefix: &str);
}

// Implemented for "empty" results that never contain values,
// but may contain errors
impl<T, E> ResultOrLog<T, E> for Result<T, E>
where
    E: Display,
{
    fn or_log_error(self, prefix: &str) {
        match self {
            Ok(_) => return,
            Err(err) => log::error!("{}: {}", prefix, err),
        }
    }

    fn or_log_warning(self, prefix: &str) {
        match self {
            Ok(_) => return,
            Err(err) => log::warn!("{}: {}", prefix, err),
        }
    }

    fn or_log_info(self, prefix: &str) {
        match self {
            Ok(_) => return,
            Err(err) => log::info!("{}: {}", prefix, err),
        }
    }
}
