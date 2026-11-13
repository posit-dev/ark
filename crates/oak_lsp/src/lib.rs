pub mod analysis;
pub mod backend;
pub mod capabilities;
pub mod config;
mod content_changes;
pub mod environments;
pub mod find_references;
pub mod goto_definition;
pub mod handlers;
#[cfg(any(test, feature = "testing"))]
pub mod harness;
mod io_pool;
pub mod main_loop;
pub mod open_file;
mod panic;
pub mod rename;
pub mod sidecar;
pub mod sources;
pub mod state;
pub mod state_handlers;
#[cfg(any(test, feature = "testing"))]
pub mod tests;
pub mod traits;
mod watchdog;

// These send LSP messages in a non-async and non-blocking way.
// The LOG level is not timestamped so we're not using it.
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)+) => ($crate::_log!(tower_lsp_server::ls_types::MessageType::INFO, $($arg)+))
}
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)+) => ($crate::_log!(tower_lsp_server::ls_types::MessageType::WARNING, $($arg)+))
}
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)+) => ($crate::_log!(tower_lsp_server::ls_types::MessageType::ERROR, $($arg)+))
}
// `cfg!(test)` is evaluated in the crate that invokes the macro, so the unit
// tests of every calling crate skip the client channel, which no unit test
// connects.
#[macro_export]
macro_rules! _log {
    ($lvl:expr, $($arg:tt)+) => ({
        if !cfg!(test) {
            $crate::main_loop::log($lvl, format!($($arg)+));
        }
    });
}

// Gated on `RUST_LOG` because the client channel has no trace level of its own.
#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)+) => ({
        if $crate::trace_enabled!() {
            $crate::_log!(tower_lsp_server::ls_types::MessageType::INFO, $($arg)+)
        }
    })
}

/// Reports whether a trace event at the invocation site would be recorded, so
/// callers can avoid constructing trace-only payloads when no layer would
/// consume them.
///
/// This must be a macro because `tracing::enabled!()` registers interest in the
/// invoking module. A function would apply module-scoped `RUST_LOG` directives
/// to `oak_lsp` instead of the caller.
#[macro_export]
macro_rules! trace_enabled {
    () => {
        tracing::enabled!(tracing::Level::TRACE)
    };
}

pub(crate) use main_loop::publish_diagnostics;
