//
// result.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

#[macro_export]
macro_rules! debug_panic {
    ( $($fmt_arg:tt)* ) => {
        if cfg!(debug_assertions) {
            panic!( $($fmt_arg)* );
        } else {
            let backtrace = std::backtrace::Backtrace::capture();
            log::error!("{}\n{:?}", format_args!($($fmt_arg)*), backtrace);
        }
    };
}

#[macro_export]
macro_rules! soft_assert {
    ( $cond:expr $(,)? ) => {
        if !$cond {
            stdext::debug_panic!("assertion failed: {}", stringify!($cond));
        }
    };
    ( $cond:expr, $($fmt_arg:tt)+ ) => {
        if !$cond {
            stdext::debug_panic!( $($fmt_arg)+ );
        }
    };
}

// From https://github.com/zed-industries/zed/blob/a910c594/crates/util/src/util.rs#L554
pub trait ResultExt<E> {
    type Ok;

    fn log_err(self) -> Option<Self::Ok>;
    /// Assert that this result should never be an error in development or tests
    fn debug_assert_ok(self, reason: &str) -> Self;
    fn warn_on_err(self) -> Option<Self::Ok>;
    fn log_with_level(self, level: log::Level) -> Option<Self::Ok>;
    fn anyhow(self) -> anyhow::Result<Self::Ok>
    where
        E: Into<anyhow::Error>;
}

impl<T, E> ResultExt<E> for Result<T, E>
where
    E: std::fmt::Debug,
{
    type Ok = T;

    #[track_caller]
    fn log_err(self) -> Option<T> {
        self.log_with_level(log::Level::Error)
    }

    #[track_caller]
    fn warn_on_err(self) -> Option<T> {
        self.log_with_level(log::Level::Warn)
    }

    #[track_caller]
    fn log_with_level(self, level: log::Level) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                let location = std::panic::Location::caller();
                let file = location.file();
                let line = location.line();
                log::logger().log(
                    &log::Record::builder()
                        // Unlike direct calls to `log::error!`, we're propagating an
                        // error object that typically contains backtrace information.
                        // The file/line information displayed by log is at the bottom,
                        // which can be hard to find in case of a long backtrace, so
                        // we mention these again before the error message to
                        // make it easier to identify where the log message was
                        // emitted from.
                        .args(format_args!("at {file}:{line}: {error:?}"))
                        .level(level)
                        .file(Some(file))
                        .line(Some(line))
                        // Can't get the module path from tracked caller so
                        // leave it blank
                        .module_path(None)
                        .build(),
                );
                None
            },
        }
    }

    #[track_caller]
    fn debug_assert_ok(self, reason: &str) -> Self {
        if let Err(error) = &self {
            debug_panic!("{reason} - {error:?}");
        }
        self
    }

    fn anyhow(self) -> anyhow::Result<T>
    where
        E: Into<anyhow::Error>,
    {
        self.map_err(Into::into)
    }
}
