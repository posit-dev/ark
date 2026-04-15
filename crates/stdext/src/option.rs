pub trait OptionExt {
    type Some;

    fn log_none(self, message: &str) -> Self;
    fn warn_on_none(self, message: &str) -> Self;
    fn log_none_with_level(self, level: log::Level, message: &str) -> Self;

    /// Assert that this option should never be `None` in development or tests
    fn debug_assert_some(self, reason: &str) -> Self;
}

impl<T> OptionExt for Option<T> {
    type Some = T;

    #[track_caller]
    fn log_none(self, message: &str) -> Self {
        self.log_none_with_level(log::Level::Error, message)
    }

    #[track_caller]
    fn warn_on_none(self, message: &str) -> Self {
        self.log_none_with_level(log::Level::Warn, message)
    }

    #[track_caller]
    fn log_none_with_level(self, level: log::Level, message: &str) -> Self {
        if self.is_none() {
            let location = std::panic::Location::caller();
            let file = location.file();
            let line = location.line();
            log::logger().log(
                &log::Record::builder()
                    .args(format_args!("at {file}:{line}: {message}"))
                    .level(level)
                    .file(Some(file))
                    .line(Some(line))
                    .module_path(None)
                    .build(),
            );
        }
        self
    }

    #[track_caller]
    fn debug_assert_some(self, reason: &str) -> Self {
        if self.is_none() {
            crate::debug_panic!("{reason}");
        }
        self
    }
}
