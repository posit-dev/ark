//
// result.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::fmt::Display;

pub trait ResultOrLog<E> {
    /// If `self` is an error, log an error, else do nothing and consume self.
    fn or_log_error(self, prefix: &str);

    /// If `self` is an error, log a warning, else do nothing and consume self.
    fn or_log_warning(self, prefix: &str);

    /// If `self` is an error, log info, else do nothing and consume self.
    fn or_log_info(self, prefix: &str);
}

// Implemented for "empty" results that never contain values,
// but may contain errors
impl<E> ResultOrLog<E> for Result<(), E>
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
