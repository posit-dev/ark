//
// log.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::fs::File;
use std::io::prelude::*;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::Once;
use std::time::SystemTime;

use chrono::DateTime;
use chrono::Utc;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref RE_ARK_BACKTRACE: Regex = Regex::new("^\\s*\\d+:\\s*[<]?ark::").unwrap();
    static ref RE_BACKTRACE_HEADER: Regex = Regex::new("^\\s*Stack\\s+backtrace:?\\s*$").unwrap();
}

fn annotate(mut message: String) -> String {
    // split into lines
    let mut lines = message.split("\n").collect::<Vec<_>>();

    let mut occurred: Option<String> = None;
    let mut backtrace_index: Option<usize> = None;

    // look for a backtrace entry for ark
    for (index, line) in lines.iter().enumerate() {
        if let Some(_) = RE_BACKTRACE_HEADER.find(line) {
            backtrace_index = Some(index);
            continue;
        }

        if let Some(_) = RE_ARK_BACKTRACE.find(line) {
            occurred = Some(lines[index..=index + 1].join("\n"));
            break;
        }
    }

    // if we found the backtrace entry, include it within the log output
    if let Some(occurred) = occurred {
        if let Some(index) = backtrace_index {
            let insertion = ["Occurred at:", occurred.as_str(), ""].join("\n");
            lines.insert(index, insertion.as_str());
            message = lines.join("\n");
        }
    }

    message
}

fn is_internal(record: &log::Record) -> bool {
    let target = record.target();

    // Known Positron crates
    let crates = ["harp", "ark", "amalthea", "stdext"];

    // Log `target:`s default to module locations, like `harp::environment`,
    // where the element before the first `::` is the crate name. So we can use that
    // as a proxy for whether or not we are in a foreign crate.
    match target.find("::") {
        // If we don't find `::`, assume we've manually set the `target:` at the log call site.
        // This may log some false positives if foreign crates set `target:`, but that seems rare.
        None => true,
        // Otherwise match the module name against our known internal ones
        Some(loc) => {
            let this = &target[0..loc];
            crates.contains(&this)
        },
    }
}

static ONCE: Once = Once::new();
static LOGGER: Logger = Logger::new();

struct LoggerInner {
    /// The log level (set with the RUST_LOG environment variable)
    level: log::Level,

    /// The file we log to.
    /// None if no log file has been specified (we log to stdout in this case).
    file: Option<File>,
}

struct Logger {
    /// A mutex to ensure that only one thread is writing to the log file at a
    /// time. Set to `None` at compile time, set to a real result during `initialize()`.
    /// Also required for interior mutability while still being able to have a static
    /// reference to supply to `log::set_logger()`.
    inner: Mutex<Option<LoggerInner>>,
}

impl Logger {
    const fn new() -> Self {
        let inner = Mutex::new(None);
        Self { inner }
    }

    fn initialize(&self, level: log::Level, file: Option<File>) {
        let mut inner = self.inner.lock().unwrap();
        *inner = Some(LoggerInner { level, file });
    }

    fn enabled(level: log::Level, metadata: &log::Metadata) -> bool {
        metadata.level() as i32 <= level as i32
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let guard = self.inner.lock().unwrap();
        let inner = guard.as_ref().unwrap();
        Logger::enabled(inner.level, metadata)
    }

    fn log(&self, record: &log::Record) {
        if !is_internal(record) && record.level() > log::Level::Warn {
            // To avoid a noisy output channel, we don't log information
            // from foreign crates unless they are warnings or errors
            return;
        }

        let mut guard = self.inner.lock().unwrap();
        let inner = guard.as_mut().unwrap();

        if !Logger::enabled(inner.level, record.metadata()) {
            return;
        }

        // Generate timestamp.
        let now: DateTime<Utc> = SystemTime::now().into();
        let timestamp = now.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);

        // Generate prefix.
        let prefix = format!(
            "{} [{}-{}] {} {}:{}",
            timestamp,
            "ark",
            "unknown", // TODO: Current user?
            record.level(),
            record.file().unwrap_or("?"),
            record.line().unwrap_or(0),
        );

        // Generate message.
        let message = format!("{}", record.args());

        // Annotate with the error location if a stack trace is available.
        let message = annotate(message);

        // Generate message to log.
        let message = format!("{prefix}: {message}");

        if let Some(file) = inner.file.as_mut() {
            // Write to log file if one is specified.
            let status = writeln!(file, "{}", message);
            if let Err(error) = status {
                eprintln!("Error writing to log file: {error:?}");
            }
        } else {
            // If no log file is specified, write to stdout.
            if record.level() == log::Level::Error {
                eprintln!("{message}");
            } else {
                println!("{message}");
            }
        }
    }

    fn flush(&self) {
        let mut guard = self.inner.lock().unwrap();
        let inner = guard.as_mut().unwrap();

        if let Some(file) = inner.file.as_mut() {
            file.flush().unwrap();
        }
    }
}

pub fn initialize(file: Option<&str>) {
    ONCE.call_once(|| {
        // Initialize the log level, using RUST_LOG.
        let level_envvar = std::env::var("RUST_LOG").unwrap_or("info".into());

        let level = match log::Level::from_str(level_envvar.as_str()) {
            Ok(level) => level,
            Err(err) => {
                eprintln!("Error parsing RUST_LOG, defaulting to `info`: {err:?}");
                log::Level::Info
            },
        };

        log::set_max_level(level.to_level_filter());

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

        LOGGER.initialize(level, file);
        log::set_logger(&LOGGER).unwrap();
    });
}
