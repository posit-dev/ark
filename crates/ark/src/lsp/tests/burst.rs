//! Replays Quarto virtual-document churn through the production event handler.
//!
//! Chunk virtual documents use unique URIs and document-height padding before
//! closing. The replay also edits one persistent virtual document and fetches
//! package sources.
//!
//! Assertions accept any background completion order but require the final
//! diagnostics and publications. Set `ARK_BURST_REPORT=1` to print timings to
//! stderr without asserting on them.
//!
//! Scale the burst and pin the worker count for a measurement run:
//!
//! ```text
//! ARK_BURST_REPORT=1 ARK_BURST_VDOCS=40 ARK_BURST_PADDING=4000 \
//!   OAK_MAX_ANALYSIS_THREADS=4 just test --no-capture test_burst_replay
//! ```
//!
//! nextest uses the dev profile. Use `--cargo-profile release` for
//! release-profile measurements.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use oak_db::OakDatabase;
use oak_scan::DbScan;
use stdext::env_flag;
use stdext::parse_positive_count;
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::DiagnosticSeverity;
use tower_lsp_server::ls_types::GotoDefinitionResponse;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::Range;
use tower_lsp_server::ls_types::Uri;

use super::source_handler::TestBehavior;
use super::source_handler::TestSourceHandler;
use super::utils::did_change;
use super::utils::did_change_workspace_folders;
use super::utils::did_close;
use super::utils::did_open;
use super::utils::goto_definition;
use super::utils::range;
use super::utils::source_scheduler_for_test;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use crate::lsp::analysis::MAX_ANALYSIS_THREADS_ENV_VAR;
use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::backend::LspRequest;
use crate::lsp::backend::LspResponse;
use crate::lsp::backend::RequestResponse;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::AuxiliaryEvent;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::TokioUnboundedSender;

const REPORT_ENV_VAR: &str = "ARK_BURST_REPORT";

const VDOCS_ENV_VAR: &str = "ARK_BURST_VDOCS";

/// Padding lines per code line. Set this to 806 to reproduce 12,093 padding
/// lines across 15 code lines.
const PADDING_ENV_VAR: &str = "ARK_BURST_PADDING";

/// Abort a stalled replay. [`GlobalState::is_settled()`] is counter-based,
/// not time-based.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Packages referenced through `::` to trigger source-pool fetches.
const DONORS: [&str; 2] = ["donor1", "donor2"];

/// Workspace document kept open so refreshes cover a non-vdoc file.
const SCRIPT: &str = "burst_script <- function() 1\nburst_script()\n";

const HELPER: &str = "burst_helper";

/// Code-relative lines used for the helper definition, probe call, and missing
/// symbol assertions.
const DEFINITION_LINE: u32 = 0;
const CALL_LINE: u32 = 1;
const MISSING_LINE: u32 = 2;

/// Maximum open one-shot vdocs. Multiple live URIs create backlog that keyed
/// replacement cannot coalesce.
const WINDOW: usize = 4;

#[tokio::test]
async fn test_burst_replay_publishes_final_diagnostics() {
    let config = BurstConfig::from_env();
    let fixture = Fixture::new();
    let mut replay = Replay::start(&fixture, &config);

    let started = Instant::now();
    let cpu_started = process_cpu_time();

    replay.enqueue_burst(&fixture, &config);
    if tokio::time::timeout(SETTLE_TIMEOUT, replay.pump_to_settled())
        .await
        .is_err()
    {
        panic!(
            "The burst never settled: {metrics:?}",
            metrics = replay.state.lsp_state().diagnostics_metrics
        );
    }

    let wall = started.elapsed();
    let cpu = cpu_started
        .zip(process_cpu_time())
        .map(|(from, to)| to - from);

    replay.report(&fixture, &config, wall, cpu);
    replay.assert_final_state(&fixture, &config);
}

/// Controls the size of the replayed burst.
struct BurstConfig {
    vdocs: usize,
    padding_ratio: usize,
}

impl BurstConfig {
    fn from_env() -> Self {
        Self {
            vdocs: env_count(VDOCS_ENV_VAR, 8),
            padding_ratio: env_count(PADDING_ENV_VAR, 40),
        }
    }

    /// Use an edit-specific missing symbol to reject stale diagnostics
    /// publications.
    fn recurring(&self, edit: usize) -> PaddedDocument {
        let missing = missing_symbol(edit);
        let code = format!("{HELPER} <- function() 1\n{HELPER}()\n{missing}\n");
        pad(&code, self.padding_ratio)
    }

    /// Give each one-shot virtual document URI-specific symbols.
    fn temporary(&self, index: usize) -> PaddedDocument {
        let code = format!("burst_chunk_{index} <- 1\nburst_absent_{index}\n");
        pad(&code, self.padding_ratio)
    }

    /// Reserve the version after all one-shot virtual-document edits.
    fn final_edit(&self) -> usize {
        self.vdocs + 1
    }
}

fn missing_symbol(edit: usize) -> String {
    format!("burst_missing_{edit}")
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

type KeyFields = (String, Option<DiagnosticSeverity>, Range);

/// Virtual-document text with Quarto-style padding and code-relative offsets.
struct PaddedDocument {
    text: String,

    code_line: u32,
}

fn pad(code: &str, ratio: usize) -> PaddedDocument {
    let padding = code.lines().count() * ratio;
    let above = padding / 2;

    let mut text: String = (0..above).map(padding_line).collect();
    text.push_str(code);
    text.extend((above..padding).map(padding_line));

    PaddedDocument {
        text,
        code_line: above as u32,
    }
}

/// Alternate blank and comment padding to exercise both forms Quarto emits.
fn padding_line(index: usize) -> &'static str {
    if index.is_multiple_of(2) {
        "#\n"
    } else {
        "\n"
    }
}

struct Fixture {
    workspace: TempDir,
    library: TempDir,
    /// Virtual-document URIs use this directory, but their text arrives through
    /// `didOpen` requests.
    vdocs: TempDir,
    handler: Arc<TestSourceHandler>,
}

impl Fixture {
    fn new() -> Self {
        let library = tempfile::tempdir().unwrap();
        for donor in DONORS {
            DescriptionWriter::new()
                .package(donor)
                .version("0.0.0")
                .built("dummy")
                .write(&library.path().join(donor));
        }

        // References through `::` make the workspace scan request donor sources.
        let workspace = tempfile::tempdir().unwrap();
        let package = workspace.path().join("myproj");
        DescriptionWriter::new()
            .package("myproj")
            .version("0.0.0")
            .write(&package);
        let uses: String = DONORS
            .iter()
            .map(|donor| format!("{donor}::foo()\n"))
            .collect();
        write_sources(&package.join("R"), &[("use.R", &uses)]);

        let behavior = DONORS
            .iter()
            .map(|donor| {
                (
                    donor.to_string(),
                    TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
                )
            })
            .collect::<HashMap<_, _>>();

        Self {
            workspace,
            library,
            vdocs: tempfile::tempdir().unwrap(),
            handler: Arc::new(TestSourceHandler::new(behavior)),
        }
    }

    fn script(&self) -> PathBuf {
        self.workspace.path().join("script.R")
    }

    /// Keep one URI stable so keyed replacement can coalesce its updates.
    fn recurring(&self) -> PathBuf {
        self.vdocs.path().join("recurring.vdoc.R")
    }

    /// Use a unique URI to prevent keyed replacement from coalescing one-shot
    /// virtual documents.
    fn temporary(&self, index: usize) -> PathBuf {
        self.vdocs.path().join(format!("{index}.vdoc.R"))
    }
}

struct Replay {
    state: GlobalState,
    aux_rx: UnboundedReceiver<AuxiliaryEvent>,

    probes: VecDeque<Probe>,
    publications: Vec<Publication>,
    latencies: Vec<Latency>,
    peak_holds: usize,

    last_mutation: Option<Instant>,

    final_version: i32,
}

struct Probe {
    enqueued: Instant,
    response_rx: UnboundedReceiver<RequestResponse>,

    definition: Range,
}

struct Publication {
    uri: Uri,
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
    at: Instant,
}

/// Probe wait and handling durations. Enqueuing the full burst first makes
/// queued time an upper bound.
struct Latency {
    queued: Duration,
    handled: Duration,
}

impl Replay {
    fn start(fixture: &Fixture, config: &BurstConfig) -> Self {
        let aux_rx = init_aux_for_test();

        let mut db = OakDatabase::new();
        db.set_library_paths(&[fixture.library.path().to_path_buf()]);

        let state = GlobalState::from_parts(
            test_client(),
            world_with_source_fetching(db),
            LspState::new(
                tokio::sync::mpsc::unbounded_channel().0,
                source_scheduler_for_test(fixture.handler.clone()),
            ),
        );

        Self {
            state,
            aux_rx,
            probes: VecDeque::new(),
            publications: Vec::new(),
            latencies: Vec::new(),
            peak_holds: 0,
            last_mutation: None,
            final_version: config.final_edit() as i32,
        }
    }

    /// Queue the full burst before processing events so notifications accumulate
    /// behind the main loop.
    fn enqueue_burst(&mut self, fixture: &Fixture, config: &BurstConfig) {
        let events_tx = self.state.events_tx();
        let recurring = fixture.recurring();

        events_tx
            .send(did_change_workspace_folders(fixture.workspace.path()))
            .unwrap();
        events_tx.send(did_open(&fixture.script(), SCRIPT)).unwrap();
        events_tx
            .send(did_open(&recurring, &config.recurring(0).text))
            .unwrap();

        let mut open: VecDeque<PathBuf> = VecDeque::new();
        for index in 0..config.vdocs {
            let temporary = fixture.temporary(index);
            events_tx
                .send(did_open(&temporary, &config.temporary(index).text))
                .unwrap();
            open.push_back(temporary);

            let edit = config.recurring(index + 1);
            events_tx
                .send(did_change(&recurring, &edit.text, (index + 1) as i32))
                .unwrap();
            self.enqueue_probe(&events_tx, &recurring, &edit);

            if open.len() > WINDOW {
                close_oldest(&events_tx, &mut open);
            }
        }
        while !open.is_empty() {
            close_oldest(&events_tx, &mut open);
        }

        let last = config.recurring(config.final_edit());
        events_tx
            .send(did_change(&recurring, &last.text, self.final_version))
            .unwrap();
    }

    /// Probe the recurring virtual document's in-buffer helper, independent of
    /// workspace-scan progress.
    fn enqueue_probe(
        &mut self,
        events_tx: &TokioUnboundedSender<Event>,
        path: &Path,
        document: &PaddedDocument,
    ) {
        let call = Position::new(document.code_line + CALL_LINE, 0);
        let (event, response_rx) = goto_definition(path, call);

        events_tx.send(event).unwrap();
        self.probes.push_back(Probe {
            enqueued: Instant::now(),
            response_rx,
            definition: range(
                (document.code_line + DEFINITION_LINE, 0),
                (document.code_line + DEFINITION_LINE, HELPER.len() as u32),
            ),
        });
    }

    /// Continue through replacement batches until [`GlobalState::is_settled()`]
    /// reports no queued or active diagnostics work.
    async fn pump_to_settled(&mut self) {
        while !self.state.is_settled() {
            let event = self.state.take_event(|_event| true).await;
            self.handle(event).await;
        }
    }

    async fn handle(&mut self, event: Event) {
        let probe = is_goto_definition(&event);
        let last_mutation = self.is_last_mutation(&event);

        let dequeued = Instant::now();
        self.state.handle_event_once(event).await;
        let handled = dequeued.elapsed();

        if probe {
            self.record_probe(dequeued, handled);
        }
        if last_mutation {
            self.last_mutation = Some(Instant::now());
        }

        self.collect_publications();
        self.peak_holds = self
            .peak_holds
            .max(self.state.world().db.outstanding_holds());
    }

    fn is_last_mutation(&self, event: &Event) -> bool {
        let Event::Lsp(LspMessage::Notification(LspNotification::DidChangeTextDocument(params))) =
            event
        else {
            return false;
        };
        params.text_document.version == self.final_version
    }

    /// Reject failed probes before recording latency, so only successful
    /// requests can appear fast.
    fn record_probe(&mut self, dequeued: Instant, handled: Duration) {
        let Some(mut probe) = self.probes.pop_front() else {
            panic!("Received a goto-definition response with no probe waiting for it");
        };

        let response = probe.response_rx.try_recv().unwrap();
        let RequestResponse::Result(Ok(LspResponse::GotoDefinition(Some(
            GotoDefinitionResponse::Link(links),
        )))) = response
        else {
            panic!("The goto-definition probe found no definition");
        };
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_range, probe.definition);

        self.latencies.push(Latency {
            queued: dequeued - probe.enqueued,
            handled,
        });
    }

    fn collect_publications(&mut self) {
        while let Ok(event) = self.aux_rx.try_recv() {
            let AuxiliaryEvent::PublishDiagnostics(publication) = event else {
                continue;
            };
            self.publications.push(Publication {
                uri: publication.uri,
                diagnostics: publication.diagnostics,
                version: publication.version,
                at: Instant::now(),
            });
        }
    }

    fn assert_final_state(&self, fixture: &Fixture, config: &BurstConfig) {
        let last = self.last_publication(&uri(&fixture.recurring()));
        assert_eq!(key_fields(&last.diagnostics), final_diagnostics(config));
        assert_eq!(last.version, Some(self.final_version));

        // Require a clearing publication because an in-flight pass can publish
        // diagnostics after `didClose`.
        for index in 0..config.vdocs {
            let temporary = uri(&fixture.temporary(index));
            assert!(self.publications.iter().any(|publication| {
                publication.uri == temporary && publication.diagnostics.is_empty()
            }));
        }

        let mut open: Vec<&str> = self
            .state
            .world()
            .open_files
            .values()
            .map(|open_file| open_file.wire_uri().as_str())
            .collect();
        open.sort();
        let mut expected = vec![
            uri(&fixture.recurring()).as_str().to_string(),
            uri(&fixture.script()).as_str().to_string(),
        ];
        expected.sort();
        assert_eq!(open, expected);

        assert!(self.probes.is_empty());
        assert_eq!(self.latencies.len(), config.vdocs);

        let diagnostics = self.state.lsp_state().diagnostics_metrics;
        assert!(diagnostics.batches >= config.vdocs as u64);
        assert!(diagnostics.results_accepted > 0);

        let queue = self.state.lsp_state().analysis_pool.metrics();
        assert_eq!(queue.waiting(), 0);
        assert_eq!(queue.running(), 0);
        assert!(queue.completed > 0);
        assert!(queue.queued >= diagnostics.tasks_queued);
    }

    #[track_caller]
    fn last_publication(&self, uri: &Uri) -> &Publication {
        let last = self
            .publications
            .iter()
            .rev()
            .find(|publication| publication.uri == *uri);

        match last {
            Some(publication) => publication,
            None => panic!("Nothing was published for {}", uri.as_str()),
        }
    }

    fn report(
        &self,
        fixture: &Fixture,
        config: &BurstConfig,
        wall: Duration,
        cpu: Option<Duration>,
    ) {
        if !env_flag(REPORT_ENV_VAR) {
            return;
        }

        let document = config.recurring(config.final_edit());
        let recurring = uri(&fixture.recurring());

        eprintln!("--- burst replay ---");
        eprintln!(
            "{vdocs} temporary vdocs, {ratio} padding lines per code line, {lines} lines per vdoc",
            vdocs = config.vdocs,
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
        eprintln!("wall {wall:.2?}, cpu {}", format_cpu(cpu));
        eprintln!(
            "final publication {}",
            match self.final_publication_delay(&recurring) {
                Some(delay) => format!("{delay:.2?} after the last mutation"),
                None => String::from("(not observed)"),
            }
        );
        eprintln!("goto-definition {}", self.format_latencies());

        let diagnostics = self.state.lsp_state().diagnostics_metrics;
        eprintln!(
            "diagnostics: {batches} batches, {tasks} tasks, {accepted} accepted, {stale} stale",
            batches = diagnostics.batches,
            tasks = diagnostics.tasks_queued,
            accepted = diagnostics.results_accepted,
            stale = diagnostics.results_stale,
        );

        let queue = self.state.lsp_state().analysis_pool.metrics();
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
        eprintln!("peak outstanding db holds: {}", self.peak_holds);
    }

    fn final_publication_delay(&self, recurring: &Uri) -> Option<Duration> {
        let mutation = self.last_mutation?;
        Some(self.last_publication(recurring).at - mutation)
    }

    fn format_latencies(&self) -> String {
        let queued = summarise(self.latencies.iter().map(|latency| latency.queued));
        let handled = summarise(self.latencies.iter().map(|latency| latency.handled));
        format!("queued {queued}, handled {handled}")
    }
}

/// Discard the oldest one-shot virtual document as chunk lifetimes overlap.
fn close_oldest(events_tx: &TokioUnboundedSender<Event>, open: &mut VecDeque<PathBuf>) {
    if let Some(oldest) = open.pop_front() {
        events_tx.send(did_close(&oldest)).unwrap();
    }
}

fn is_goto_definition(event: &Event) -> bool {
    matches!(
        event,
        Event::Lsp(LspMessage::Request(LspRequest::GotoDefinition(_), _))
    )
}

fn uri(path: &Path) -> Uri {
    Uri::from_file_path(path).unwrap()
}

/// Use the final edit's unique symbol to reject stale publications.
fn final_diagnostics(config: &BurstConfig) -> Vec<KeyFields> {
    let document = config.recurring(config.final_edit());
    let symbol = missing_symbol(config.final_edit());
    let line = document.code_line + MISSING_LINE;

    vec![(
        format!("No symbol named '{symbol}' in scope."),
        Some(DiagnosticSeverity::WARNING),
        range((line, 0), (line, symbol.len() as u32)),
    )]
}

/// Extract the fields asserted by this replay without pinning unrelated
/// protocol fields.
fn key_fields(diagnostics: &[Diagnostic]) -> Vec<KeyFields> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.message.clone(),
                diagnostic.severity,
                diagnostic.range,
            )
        })
        .collect()
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
