//
// logger.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use once_cell::sync::OnceCell;
use regex::Regex;
use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

use crate::logger_hprof;

/// Every first-party crate's directory name under `crates/`, baked in by
/// `build.rs` so [`internal_crates()`] doesn't need a hardcoded list that
/// goes stale as crates are added.
fn internal_crates() -> impl Iterator<Item = &'static str> {
    env!("ARK_INTERNAL_CRATES").split(',')
}

pub fn init(log_file: Option<&str>, profile_file: Option<&str>) {
    static ONCE: Once = Once::new();

    ONCE.call_once(|| {
        // Parse `RUST_LOG`
        let mut env_filter = EnvFilter::from_default_env();

        // Propagate 'ark' verbosity to internal crates
        let re = Regex::new(r"ark=([a-zA-Z]+)(,|$)").unwrap();
        let rust_log = std::env::var("RUST_LOG")
            .ok()
            .unwrap_or_else(|| String::from("ark=info"));
        if let Some(level) = re
            .captures(&rust_log)
            .and_then(|c| c.get(1))
            .map(|c| c.as_str())
        {
            for pkg in internal_crates().filter(|&pkg| pkg != "ark") {
                if let Ok(directive) = format!("{pkg}={level}").parse() {
                    env_filter = env_filter.add_directive(directive);
                }
            }
        }

        // Spawn appender thread for non-blocking writes
        static LOG_GUARD: OnceCell<WorkerGuard> = OnceCell::new();
        let log_writer = non_blocking(log_file, &LOG_GUARD);

        let log = tracing_subscriber::fmt::layer()
            // Use pretty representation. This has more spacing
            // and a clearer layout for fields.
            .pretty()
            // Disable ANSI escapes, those are not supported in Code
            .with_ansi(false)
            // Display source code file paths
            .with_file(true)
            // Display source code line numbers
            .with_line_number(true)
            // Don't display the thread ID
            .with_thread_ids(false)
            // Don't display the event's target (module path).
            // Mostly redundant with file paths.
            .with_target(false)
            // Use our custom file writer
            .with_writer(log_writer)
            // Filter based on `RUST_LOG` envvar
            .with_filter(env_filter);

        let subscriber = tracing_subscriber::Registry::default()
            .with(log)
            .with(errors_layer());

        // Only log profile if requested
        if profile_file.is_some() {
            static PROFILE_GUARD: OnceCell<WorkerGuard> = OnceCell::new();
            let profile_writer = non_blocking(profile_file, &PROFILE_GUARD);

            // Profile anything taking over 50ms by default
            let config = std::env::var("ARK_PROFILE").unwrap_or("*>50".into());

            let profile = logger_hprof::layer(&config, profile_writer);
            subscriber.with(profile).try_init().unwrap();
        } else {
            subscriber.try_init().unwrap();
        }
    });
}

/// Adds span context to errors without enabling trace events.
///
/// [`tracing_error::ErrorLayer`] does not filter its own interest. Without this
/// span-only filter, it reports
/// [`Interest::always()`](tracing::subscriber::Interest::always) for every level,
/// so `tracing::enabled!(tracing::Level::TRACE)` and
/// [`crate::lsp::log_trace!`] report enabled regardless of the log layer's
/// `RUST_LOG`-derived [`EnvFilter`]. `SpanTrace` only requires span metadata.
fn errors_layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    tracing_error::ErrorLayer::default()
        .with_filter(filter::filter_fn(|metadata| metadata.is_span()))
}

// Returns a boxed value for genericity
fn non_blocking(file: Option<&str>, cell: &OnceCell<WorkerGuard>) -> BoxMakeWriter {
    let file = file.and_then(|file| {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(file)
            .ok()
    });

    if let Some(file) = file {
        let (writer, guard) = tracing_appender::non_blocking(file);

        // Save the guard forever
        cell.set(guard).unwrap();

        BoxMakeWriter::new(writer)
    } else {
        BoxMakeWriter::new(std::io::stderr)
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::EnvFilter;

    use super::errors_layer;
    use super::init;
    use super::internal_crates;

    /// These two tests exercise `init()`'s actual layer composition (the `fmt`
    /// layer plus [`errors_layer()`]), rather than the minimal registry in
    /// `test_trace_events_follow_env_filter_with_errors_layer_present` below,
    /// so a future layer added to `init()` without a filter is caught here too.
    #[test]
    fn test_log_trace_disabled_under_production_subscriber_default() {
        std::env::set_var("RUST_LOG", "ark=info");
        init(None, None);
        assert!(!crate::lsp::trace_enabled!());
    }

    #[test]
    fn test_log_trace_enabled_under_production_subscriber_with_ark_trace() {
        std::env::set_var("RUST_LOG", "ark=trace");
        init(None, None);
        assert!(crate::lsp::trace_enabled!());
    }

    #[test]
    fn test_trace_events_follow_env_filter_with_errors_layer_present() {
        let info = tracing_subscriber::registry()
            .with(EnvFilter::new("ark=info"))
            .with(errors_layer());
        tracing::subscriber::with_default(info, || {
            assert!(!tracing::enabled!(tracing::Level::TRACE));
        });

        let trace = tracing_subscriber::registry()
            .with(EnvFilter::new("ark=trace"))
            .with(errors_layer());
        tracing::subscriber::with_default(trace, || {
            assert!(tracing::enabled!(tracing::Level::TRACE));
        });
    }

    #[test]
    fn test_internal_crates_includes_path_crates() {
        let crates: Vec<&str> = internal_crates().collect();
        assert!(crates.contains(&"oak_db"));
        assert!(crates.contains(&"oak_semantic"));
        assert!(crates.contains(&"amalthea"));
        assert!(crates.contains(&"harp"));
        assert!(crates.contains(&"stdext"));
    }

    #[test]
    fn test_internal_crates_excludes_git_dependencies() {
        let crates: Vec<&str> = internal_crates().collect();
        assert!(!crates.contains(&"aether_factory"));
        assert!(!crates.contains(&"aether_syntax"));
    }
}
