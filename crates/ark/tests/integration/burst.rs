//! Checks the burst replay's final diagnostics, navigation answers, and
//! publications. Set `ARK_BURST_REPORT=1` to print timings to stderr without
//! asserting on them.
//!
//! Scale the burst and pin the worker count for a measurement run.
//!
//! ```text
//! ARK_BURST_REPORT=1 ARK_BURST_TEMPORARIES=40 ARK_BURST_PADDING=4000 \
//!   OAK_MAX_ANALYSIS_THREADS=4 just test --no-capture test_burst_replay
//! ```
//!
//! nextest uses the dev profile. Use `--cargo-profile release` for
//! release-profile measurements, or the `burst` benchmark case for the
//! fixed default size.

use std::time::Duration;
use std::time::Instant;

use burst_replay::BurstConfig;
use burst_replay::Fixture;
use burst_replay::Replay;
use stdext::env_flag;
use stdext::parse_positive_count;

#[path = "../support/burst_replay.rs"]
mod burst_replay;

const REPORT_ENV_VAR: &str = "ARK_BURST_REPORT";

const TEMPORARIES_ENV_VAR: &str = "ARK_BURST_TEMPORARIES";

const PADDING_ENV_VAR: &str = "ARK_BURST_PADDING";

const MAX_ANALYSIS_THREADS_ENV_VAR: &str = "OAK_MAX_ANALYSIS_THREADS";

/// Abort a stalled replay. [`ark::lsp::harness::LspSession::is_settled()`] is
/// counter-based, not time-based.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test]
async fn test_burst_replay_publishes_final_diagnostics() {
    let config = config_from_env();
    let fixture = Fixture::new();
    let mut replay = Replay::start(&fixture, config).await;
    replay.enqueue_burst(&fixture);

    let started = Instant::now();
    let cpu_started = process_cpu_time();

    if tokio::time::timeout(SETTLE_TIMEOUT, replay.drive_to_endpoint(&fixture))
        .await
        .is_err()
    {
        panic!(
            "The burst never reached its endpoint: {metrics:?}",
            metrics = replay.session.diagnostics_metrics()
        );
    }
    let endpoint = started.elapsed();

    if tokio::time::timeout(SETTLE_TIMEOUT, replay.settle())
        .await
        .is_err()
    {
        panic!(
            "The burst never settled: {metrics:?}",
            metrics = replay.session.diagnostics_metrics()
        );
    }
    let settled = started.elapsed();
    let cpu = cpu_started
        .zip(process_cpu_time())
        .map(|(from, to)| to - from);

    replay.assert_final_state(&fixture);
    report(&replay, endpoint, settled, cpu);
}

fn config_from_env() -> BurstConfig {
    BurstConfig {
        temporaries: env_count(TEMPORARIES_ENV_VAR, BurstConfig::DEFAULT.temporaries),
        padding_ratio: env_count(PADDING_ENV_VAR, BurstConfig::DEFAULT.padding_ratio),
    }
}

fn env_count(var: &str, default: usize) -> usize {
    let Ok(value) = std::env::var(var) else {
        return default;
    };
    match parse_positive_count(&value) {
        Some(count) => count,
        None => panic!("{var}={value} is not a positive integer"),
    }
}

fn report(replay: &Replay, endpoint: Duration, settled: Duration, cpu: Option<Duration>) {
    if !env_flag(REPORT_ENV_VAR) {
        return;
    }

    let config = replay.config;
    let document = config.recurring(config.final_edit());

    eprintln!("--- burst replay ---");
    eprintln!(
        "{temporaries} temporary documents, {ratio} padding lines per code line, {lines} lines per document",
        temporaries = config.temporaries,
        ratio = config.padding_ratio,
        lines = document.text.lines().count(),
    );
    eprintln!(
        "{threads} analysis threads",
        threads = match std::env::var(MAX_ANALYSIS_THREADS_ENV_VAR) {
            Ok(threads) => threads,
            Err(_) => String::from("default"),
        }
    );
    // Settlement time includes scans, source ingestion, and refreshes that
    // finish after the endpoint, exposing work the endpoint timing excludes.
    eprintln!(
        "final diagnostics and probes {endpoint:.2?}, settled {settled:.2?}, cpu {}",
        format_cpu(cpu)
    );
    eprintln!("goto-definition {}", format_latencies(replay));

    let diagnostics = replay.session.diagnostics_metrics();
    eprintln!(
        "diagnostics: {batches} batches, {tasks} tasks, {accepted} accepted, {stale} stale",
        batches = diagnostics.batches,
        tasks = diagnostics.tasks_queued,
        accepted = diagnostics.results_accepted,
        stale = diagnostics.results_stale,
    );

    let queue = replay.session.analysis_metrics();
    eprintln!(
        "queue: {queued} queued, {replaced} replaced, {started} started, {completed} completed, \
         {cancelled_queued} dropped before start, {cancelled_running} cancelled mid-pass, peak depth {peak}",
        queued = queue.queued,
        replaced = queue.replaced,
        started = queue.started,
        completed = queue.completed,
        cancelled_queued = queue.cancelled_queued,
        cancelled_running = queue.cancelled_running,
        peak = queue.peak_queue_len,
    );
    eprintln!(
        "peak outstanding db holds: {}",
        replay.session.peak_outstanding_holds()
    );
}

/// Probe latency runs from sending until the probe's handler returns, so it
/// includes the remaining burst-enqueue time before the loop starts and
/// earlier event handling. Handler duration excludes enqueueing and queue
/// wait.
fn format_latencies(replay: &Replay) -> String {
    let answers: Vec<_> = replay
        .probes
        .iter()
        .filter_map(|probe| replay.session.definition_answer(probe.request))
        .collect();

    let latency = summarise(answers.iter().map(|answer| answer.latency));
    let handled = summarise(answers.iter().map(|answer| answer.handled));
    format!("latency {latency}, handled {handled}")
}

fn summarise(durations: impl Iterator<Item = Duration>) -> String {
    let mut durations: Vec<Duration> = durations.collect();
    durations.sort();

    let Some(max) = durations.last() else {
        return String::from("(none)");
    };
    let median = durations[durations.len() / 2];
    format!("median {median:.2?}, max {max:.2?}")
}

fn format_cpu(cpu: Option<Duration>) -> String {
    match cpu {
        Some(cpu) => format!("{cpu:.2?}"),
        None => String::from("(unavailable on this platform)"),
    }
}

/// Return process user and system time, including worker threads, or `None`
/// where unavailable.
#[cfg(unix)]
fn process_cpu_time() -> Option<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();

    // SAFETY: `getrusage()` initializes the supplied `rusage`. `assume_init()`
    // runs only after it succeeds.
    let usage = unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
            return None;
        }
        usage.assume_init()
    };

    Some(timeval(usage.ru_utime) + timeval(usage.ru_stime))
}

#[cfg(unix)]
fn timeval(time: libc::timeval) -> Duration {
    Duration::from_secs(time.tv_sec as u64) + Duration::from_micros(time.tv_usec as u64)
}

#[cfg(not(unix))]
fn process_cpu_time() -> Option<Duration> {
    None
}
