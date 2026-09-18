//
// log_capture.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

static MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOGGER: CapturingLogger = CapturingLogger;

struct CapturingLogger;

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        MESSAGES.lock().unwrap().push(record.args().to_string());
    }

    fn flush(&self) {}
}

/// Install a process-wide capture of `log` crate records.
///
/// `log`'s global logger can only be set once per process, so call this at
/// most once. Production code never installs a logger, so this is safe from
/// a single test as long as `nextest` runs it in its own process.
pub fn install_log_capture() {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
}

/// Wait for a captured record containing `substring`.
#[track_caller]
pub fn wait_for_captured_log(substring: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;

    loop {
        if MESSAGES
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.contains(substring))
        {
            return;
        }

        if Instant::now() >= deadline {
            panic!("No captured log message containing {substring:?} within {timeout:?}");
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}
