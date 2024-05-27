//
// logger.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use regex::Regex;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::EnvFilter;

pub fn init(file: Option<&str>) {
    static ONCE: Once = Once::new();

    ONCE.call_once(|| {
        let file = match file {
            None => None,
            Some(file) => {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .append(true)
                    .create(true)
                    .open(file);

                match file {
                    Ok(file) => Some(file),
                    Err(error) => {
                        eprintln!("Error initializing log: {error:?}");
                        None
                    },
                }
            },
        };

        // Spawn appender thread for non-blocking writes
        let (writer, guard) = if let Some(file) = file {
            tracing_appender::non_blocking(file)
        } else {
            tracing_appender::non_blocking(std::io::stderr())
        };

        static LOG_GUARD: std::sync::OnceLock<WorkerGuard> = std::sync::OnceLock::new();
        LOG_GUARD.set(guard).unwrap();

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

        let subscriber = tracing_subscriber::fmt::fmt()
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
            .with_writer(BoxMakeWriter::new(writer))
            // Filter based on `RUST_LOG` envvar
            .with_env_filter(env_filter)
            .finish();

        tracing::subscriber::set_global_default(subscriber).unwrap();

        // Propagate events from log:: crate as tracing:: events
        tracing_log::LogTracer::builder().init().unwrap();
    });
}
