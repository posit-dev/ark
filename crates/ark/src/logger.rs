//
// logger.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use once_cell::sync::OnceCell;
use regex::Regex;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

use crate::logger_hprof;

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
            for pkg in vec!["amalthea", "harp", "stdext"] {
                if let Ok(directive) = format!("{pkg}={level}").parse() {
                    env_filter = env_filter.add_directive(directive);
                }
            }
        }

        // Spawn appender thread for non-blocking writes
        static mut LOG_GUARD: OnceCell<WorkerGuard> = OnceCell::new();
        let log_writer = non_blocking(log_file, unsafe { &mut LOG_GUARD });

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

        let subscriber = tracing_subscriber::Registry::default().with(log);

        // Only log profile if requested
        if profile_file.is_some() {
            static mut PROFILE_GUARD: OnceCell<WorkerGuard> = OnceCell::new();
            let profile_writer = non_blocking(profile_file, unsafe { &mut PROFILE_GUARD });

            // Profile anything taking over 50ms by default
            let config = std::env::var("ARK_PROFILE").unwrap_or("*>50".into());

            let profile = logger_hprof::layer(&config, profile_writer);
            subscriber.with(profile).try_init().unwrap();
        } else {
            subscriber.try_init().unwrap();
        }
    });
}

// Returns a boxed value for genericity
fn non_blocking(file: Option<&str>, cell: &mut OnceCell<WorkerGuard>) -> BoxMakeWriter {
    let file = file.and_then(|file| {
        std::fs::OpenOptions::new()
            .write(true)
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
