//! End-to-end measurement through the production main loop.
//!
//! [`LspSession`] consumes an [`LspHarness`] and drives production event handling
//! against a simulated editor without rebuilding its database. Editor operations
//! bypass incoming JSON-RPC parsing, while server-to-editor traffic uses the
//! production client path.
//!
//! Only one session may exist per process because the auxiliary sender is
//! process-global. Starting a new session retires the previous one's auxiliary
//! receiver, so sessions must run one after another.

mod package_sources;
#[cfg(test)]
mod test_access;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use aether_path::FilePath;
use anyhow::anyhow;
use package_sources::PackageSources;
use serde_json::Value;
use tokio::sync::mpsc::error::TryRecvError;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::GotoDefinitionResponse;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::Uri;

use super::editor::client::TestClient;
use super::editor::notifications;
use super::LspHarness;
use crate::lsp::analysis::DiagnosticsMetrics;
use crate::lsp::analysis::PoolMetrics;
use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspRequest;
use crate::lsp::backend::LspResponse;
use crate::lsp::backend::RequestResponse;
use crate::lsp::main_loop::register_auxiliary_tx;
use crate::lsp::main_loop::AuxiliaryEvent;
use crate::lsp::main_loop::DiagnosticsPublication;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::TokioUnboundedReceiver;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceScheduler;
use crate::lsp::traits::url::UrlExt;

impl LspHarness {
    /// Start the main loop against a simulated editor that serves `settings`
    /// for `workspace/configuration`. Source fetching stays disabled.
    ///
    /// The returned session has completed the startup handshake and settled
    /// the work it scheduled, so a measured section begins from an idle loop
    /// rather than racing startup diagnostics. Consuming the harness prevents
    /// buffers from being prepared outside the main loop after the session
    /// starts.
    pub async fn start(self, settings: &[(&str, Value)]) -> LspSession {
        self.start_with(settings, None).await
    }

    /// Serve source requests from prewritten `R/` directories, keyed by package
    /// name. Requests use the source scheduler, I/O pool, and ingestion path,
    /// but do not download or write source files. Unknown packages return a
    /// source-fetch failure.
    pub async fn start_with_package_sources(
        self,
        settings: &[(&str, Value)],
        directories: HashMap<String, PathBuf>,
    ) -> LspSession {
        let handler = Arc::new(PackageSources::new(directories));
        self.start_with(settings, Some(handler)).await
    }

    async fn start_with(
        self,
        settings: &[(&str, Value)],
        source_handler: Option<Arc<dyn SourceHandler>>,
    ) -> LspSession {
        let client = TestClient::new(settings).await;

        // Register before constructing the session because the main loop may
        // publish on its first tick.
        let (auxiliary_tx, auxiliary_rx) = tokio::sync::mpsc::unbounded_channel();
        register_auxiliary_tx(auxiliary_tx);

        // Read the folders before `self.state` moves into the loop, so the
        // `initialize` request re-declares the workspace the caller prepared
        // rather than clearing it.
        let folders: Vec<Uri> = self
            .state
            .workspace
            .folders
            .iter()
            .map(|folder| folder.to_url().to_uri().unwrap())
            .collect();

        let state = GlobalState::from_parts(
            client.client(),
            self.state,
            LspState::new(
                tokio::sync::mpsc::unbounded_channel().0,
                SourceScheduler::new(source_handler),
            ),
        );

        let mut session = LspSession {
            state,
            client,
            auxiliary_rx,
            raw_publications: Vec::new(),
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            definition_answers: Vec::new(),
            pending_definitions: Vec::new(),
            peak_outstanding_holds: 0,
            background_panics: 0,
        };

        session.handshake(folders).await;
        session
    }
}

/// How often [`LspSession::next_event()`] re-checks settlement while no event
/// arrives. The analysis pool can go idle without sending one, for instance
/// when its last task was cancelled, so waiting on channels alone could hang.
const SETTLE_POLL: Duration = Duration::from_millis(1);

/// Keeps each selected result owned while [`LspSession::next_event()`] borrows
/// several fields. Keep [`Event`] inline because boxing it would add an
/// allocation to every main-loop event.
#[expect(clippy::large_enum_variant)]
enum Wakeup {
    Event(Event),
    Auxiliary(AuxiliaryEvent),
    Tick,
}

/// A production main loop connected to a simulated editor.
pub struct LspSession {
    state: GlobalState,
    client: TestClient,

    auxiliary_rx: TokioUnboundedReceiver<AuxiliaryEvent>,

    /// Main-loop publications before unchanged diagnostics are suppressed.
    raw_publications: Vec<DiagnosticsPublication>,

    /// Tags this session's [`DefinitionRequest`]s so another session can
    /// reject them.
    id: u64,

    /// Answers indexed by [`DefinitionRequest`], `None` until collected.
    definition_answers: Vec<Option<DefinitionAnswer>>,

    /// Requests still awaiting an answer. Collection after each event visits
    /// only these, so its cost tracks outstanding requests rather than every
    /// request sent.
    pending_definitions: Vec<PendingDefinition>,

    peak_outstanding_holds: usize,

    background_panics: usize,
}

/// Source of [`LspSession::id`]. Sessions run one after another, but a handle
/// can outlive its session, so ids must not repeat within the process.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(0);

/// Identifies a request sent with [`LspSession::send_goto_definition()`].
#[derive(Debug, Clone, Copy)]
pub struct DefinitionRequest {
    session: u64,
    index: usize,
}

/// A `textDocument/definition` answer as the session observed it.
///
/// Answers are timestamped after each main-loop handler returns and auxiliary
/// events are collected, not when the server sends them. Even synchronous
/// replies include that intervening work in their measured latency.
#[derive(Debug)]
pub struct DefinitionAnswer {
    pub response: anyhow::Result<Option<GotoDefinitionResponse>>,

    /// From sending the request until the answer was observed, including the
    /// time the request waited behind earlier events.
    pub latency: Duration,

    /// Duration of the definition-request handler immediately preceding answer
    /// collection, or `None` if a different kind of event was handled. For an
    /// asynchronous reply, this need not be the handler that produced it.
    pub handled: Option<Duration>,
}

struct PendingDefinition {
    index: usize,
    sent_at: Instant,
    response_rx: TokioUnboundedReceiver<RequestResponse>,
}

/// A publication before unchanged diagnostic sets are suppressed, including
/// accepted diagnostics and the empty sets sent by `didClose`.
#[derive(Debug, Clone, Copy)]
pub struct Publication<'session>(&'session DiagnosticsPublication);

impl<'session> Publication<'session> {
    pub fn uri(&self) -> &'session Uri {
        &self.0.uri
    }

    pub fn version(&self) -> Option<i32> {
        self.0.version
    }

    pub fn diagnostics(&self) -> &'session [Diagnostic] {
        &self.0.diagnostics
    }
}

impl LspSession {
    /// Run the production startup handshake, then settle the work it
    /// schedules.
    ///
    /// `initialize` negotiates capabilities and dispatches a workspace scan
    /// over `folders`, and `initialized` registers capabilities, pulls the
    /// settings, and releases the source scheduler's startup gate. Settling
    /// here means a measured section starts from an idle loop instead of
    /// racing the diagnostics that applying configuration schedules.
    async fn handshake(&mut self, folders: Vec<Uri>) {
        let (event, mut response_rx) = notifications::initialize(folders);
        self.handle_once(event).await;

        // Read the answer rather than dropping the receiver, so a rejected
        // `initialize` fails here instead of leaving the session running
        // against a half-configured loop.
        match response_rx.recv().await {
            Some(RequestResponse::Result(Ok(LspResponse::Initialize(_)))) => {},
            Some(RequestResponse::Result(Err(err))) => {
                panic!("The main loop rejected `initialize`: {err:?}")
            },
            _ => panic!("The main loop did not answer `initialize`"),
        }

        self.handle_once(notifications::initialized()).await;

        self.settle().await;
    }

    /// The main loop only publishes diagnostics while handling an event, so
    /// collecting auxiliary events here records every publication.
    pub(crate) async fn handle_once(&mut self, event: Event) {
        let is_definition = matches!(
            event,
            Event::Lsp(LspMessage::Request(LspRequest::GotoDefinition(_), _))
        );

        let started = Instant::now();
        self.state.handle_event_once(event).await;
        let handled = started.elapsed();

        self.collect_auxiliary();
        self.collect_definition_answers(is_definition.then_some(handled));

        self.peak_outstanding_holds = self
            .peak_outstanding_holds
            .max(self.state.world().db.outstanding_holds());
    }

    /// Queue `didOpen` for the main loop to pick up, the way an editor does.
    /// Unlike [`LspHarness::prepare_document()`], which only registers the
    /// buffer, this runs the open handler and the diagnostics it schedules.
    pub fn send_did_open(&self, path: &Path, contents: &str) {
        self.enqueue(notifications::did_open(path, contents));
    }

    /// Queue a whole-document `didChange` at `version`, which must exceed the
    /// version the document was opened at.
    ///
    /// Queueing rather than handling inline keeps main-loop dispatch inside a
    /// measured section, and lets a caller stack several changes to exercise
    /// keyed replacement.
    pub fn send_did_change(&self, path: &Path, contents: &str, version: i32) {
        self.enqueue(notifications::did_change(path, contents, version));
    }

    pub fn send_did_close(&self, path: &Path) {
        self.enqueue(notifications::did_close(path));
    }

    /// Queue a `didChangeWorkspaceFolders` adding `path`, which schedules a
    /// workspace scan once handled.
    pub fn send_did_change_workspace_folders(&self, path: &Path) {
        self.enqueue(notifications::did_change_workspace_folders(path));
    }

    /// Queue a `textDocument/definition` request. Read its answer with
    /// [`Self::definition_answer()`] once the loop has handled it.
    pub fn send_goto_definition(&mut self, path: &Path, position: Position) -> DefinitionRequest {
        let (event, response_rx) = notifications::goto_definition(path, position);
        let index = self.definition_answers.len();

        self.definition_answers.push(None);
        self.pending_definitions.push(PendingDefinition {
            index,
            sent_at: Instant::now(),
            response_rx,
        });
        self.enqueue(event);

        DefinitionRequest {
            session: self.id,
            index,
        }
    }

    /// Return the observed answer, or `None` until the session collects it.
    /// Panics if `request` was sent by another session.
    pub fn definition_answer(&self, request: DefinitionRequest) -> Option<&DefinitionAnswer> {
        if request.session != self.id {
            panic!("{request:?} was sent by another session");
        }
        self.definition_answers
            .get(request.index)
            .and_then(Option::as_ref)
    }

    /// Queue an event without handling it, to model a busy main loop.
    pub(crate) fn enqueue(&self, event: Event) {
        self.state.events_tx().send(event).unwrap();
    }

    /// Pump the main loop until it accepts diagnostics for `path` at
    /// `version`, and hand them back.
    ///
    /// This covers the full round trip through the event loop, the diagnostics
    /// scheduler, the analysis pool, and generation filtering. It stops before
    /// the auxiliary loop, so it leaves out deduplication and the client
    /// notification. Publications already recorded when the wait starts don't
    /// count, so an earlier round trip over the same document can't satisfy
    /// it.
    ///
    /// A settled loop has no work left that could produce the publication, so
    /// reaching that state without it is a failure rather than a longer wait.
    pub async fn wait_for_accepted_diagnostics(
        &mut self,
        path: &Path,
        version: i32,
    ) -> Vec<Diagnostic> {
        let accepted = self
            .wait_for_all_accepted_diagnostics(&[(path, version)])
            .await;
        let Some(diagnostics) = accepted.into_iter().next() else {
            panic!("Expected one set of diagnostics per target");
        };
        diagnostics
    }

    /// Like [`Self::wait_for_accepted_diagnostics()`], for several documents at
    /// once. Returns each target's diagnostics in the order given, and returns
    /// as soon as every target has one publication, even if later refreshes of
    /// the same documents are still queued.
    pub async fn wait_for_all_accepted_diagnostics(
        &mut self,
        targets: &[(&Path, i32)],
    ) -> Vec<Vec<Diagnostic>> {
        let targets: Vec<(FilePath, i32)> = targets
            .iter()
            .map(|(path, version)| {
                let Some(file_path) = FilePath::from_path_buf(path.to_path_buf()) else {
                    panic!("Not an absolute UTF-8 path: {}", path.display());
                };
                (file_path, *version)
            })
            .collect();

        let mut accepted: Vec<Option<Vec<Diagnostic>>> = vec![None; targets.len()];
        let mut scanned = self.raw_publications.len();

        loop {
            for publication in &self.raw_publications[scanned..] {
                for (slot, (path, version)) in accepted.iter_mut().zip(&targets) {
                    if slot.is_none() &&
                        publication.path == *path &&
                        publication.version == Some(*version)
                    {
                        *slot = Some(publication.diagnostics.clone());
                    }
                }
            }
            scanned = self.raw_publications.len();

            if accepted.iter().all(Option::is_some) {
                return accepted.into_iter().flatten().collect();
            }

            let Some(event) = self.next_event().await else {
                let missing: Vec<_> = targets
                    .iter()
                    .zip(&accepted)
                    .filter(|(_target, slot)| slot.is_none())
                    .map(|(target, _slot)| target)
                    .collect();
                panic!("The main loop settled without accepting diagnostics for {missing:?}");
            };
            self.handle_once(event).await;
        }
    }

    /// Process queued events until no scheduler or analysis work remains.
    ///
    /// Settlement also fails on panics, unbalanced pool counters, or requests
    /// unsupported by the simulated editor.
    pub async fn settle(&mut self) {
        while let Some(event) = self.next_event().await {
            self.handle_once(event).await;
        }
    }

    /// Report whether the loop has no scheduler, analysis, or queued work
    /// left, so a caller can assert an idle starting point before measuring.
    pub fn is_settled(&self) -> bool {
        self.state.is_settled()
    }

    /// Return the next unhandled main-loop event, or `None` once the loop
    /// settles. Auxiliary events are recorded while waiting, and settlement is
    /// re-checked every [`SETTLE_POLL`].
    pub(crate) async fn next_event(&mut self) -> Option<Event> {
        loop {
            // A panicking worker can leave its scheduler pending, so the loop
            // would never settle. Inspect failures after every wakeup.
            self.check_background_failures();

            if self.state.is_settled() {
                return None;
            }

            // Limit the select's borrows so recording can reborrow `self`.
            let wakeup = tokio::select! {
                event = self.state.next_event() => Wakeup::Event(event),
                Some(event) = self.auxiliary_rx.recv() => Wakeup::Auxiliary(event),
                _ = tokio::time::sleep(SETTLE_POLL) => Wakeup::Tick,
            };

            match wakeup {
                Wakeup::Event(event) => return Some(event),
                Wakeup::Auxiliary(event) => self.record_auxiliary(event),
                Wakeup::Tick => {},
            }
        }
    }

    /// Diagnostics publications in the order the main loop made them.
    pub fn publications(&self) -> impl DoubleEndedIterator<Item = Publication<'_>> {
        self.raw_publications.iter().map(Publication)
    }

    /// Wire URIs of the documents the server holds open.
    pub fn open_documents(&self) -> Vec<&Uri> {
        self.state
            .world()
            .open_files
            .values()
            .map(|open_file| open_file.wire_uri())
            .collect()
    }

    pub fn diagnostics_metrics(&self) -> DiagnosticsMetrics {
        self.state.lsp_state().diagnostics_metrics
    }

    pub fn analysis_metrics(&self) -> PoolMetrics {
        self.state.lsp_state().analysis_pool.metrics()
    }

    /// Maximum database-handle count sampled after event handling. Peaks within
    /// a handler or between samples are not captured. Counts above 1 mean a
    /// database write must wait for other handles to drop.
    pub fn peak_outstanding_holds(&self) -> usize {
        self.peak_outstanding_holds
    }

    fn collect_definition_answers(&mut self, definition_handled: Option<Duration>) {
        let observed_at = Instant::now();
        let answers = &mut self.definition_answers;

        self.pending_definitions.retain_mut(|pending| {
            let response = match pending.response_rx.try_recv() {
                Ok(response) => definition_response(response),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => Err(anyhow!(
                    "The main loop dropped a definition request without answering"
                )),
            };
            answers[pending.index] = Some(DefinitionAnswer {
                response,
                latency: observed_at - pending.sent_at,
                handled: definition_handled,
            });
            false
        });
    }

    fn collect_auxiliary(&mut self) {
        while let Ok(event) = self.auxiliary_rx.try_recv() {
            self.record_auxiliary(event);
        }
    }

    fn record_auxiliary(&mut self, event: AuxiliaryEvent) {
        match event {
            AuxiliaryEvent::PublishDiagnostics(publication) => {
                self.raw_publications.push(publication)
            },
            AuxiliaryEvent::ReportBackgroundPanic => self.background_panics += 1,
            AuxiliaryEvent::Log(_level, _message) => {},
            AuxiliaryEvent::Shutdown => {},
            #[cfg(feature = "testing")]
            AuxiliaryEvent::TestPanic => {},
        }
    }

    #[track_caller]
    fn check_background_failures(&self) {
        let metrics = self.analysis_metrics();

        if self.background_panics > 0 || metrics.panicked > 0 {
            panic!(
                "The session saw {reports} background failure reports and {panicked} \
                 panicking analysis tasks: {metrics:?}",
                reports = self.background_panics,
                panicked = metrics.panicked,
            );
        }

        let unexpected: Vec<_> = self
            .client
            .answered_requests()
            .into_iter()
            .filter_map(|request| request.err())
            .collect();
        if !unexpected.is_empty() {
            panic!("The server sent requests the simulated editor does not handle: {unexpected:?}");
        }

        // Negative derived counters would make inconsistent bookkeeping appear
        // as an empty pool.
        if metrics.waiting() < 0 || metrics.running() < 0 {
            panic!("The analysis pool's counters are unbalanced: {metrics:?}");
        }
    }
}

fn definition_response(
    response: RequestResponse,
) -> anyhow::Result<Option<GotoDefinitionResponse>> {
    match response {
        RequestResponse::Result(Ok(LspResponse::GotoDefinition(response))) => Ok(response),
        RequestResponse::Result(Ok(response)) => Err(anyhow!(
            "Unexpected response to a definition request: {response:?}"
        )),
        RequestResponse::Result(Err(err)) => Err(anyhow!("The definition request failed: {err:?}")),
        RequestResponse::Disabled => Err(anyhow!("The definition request is disabled")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    use aether_path::FilePath;
    use oak_db::OakDatabase;
    use serde_json::Value;
    use tower_lsp_server::ls_types::Diagnostic;
    use tower_lsp_server::ls_types::Position;

    use super::LspSession;
    use crate::lsp::analysis::DiagnosticsReady;
    use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_SETTING;
    use crate::lsp::config::R_DIAGNOSTICS_ENABLED_SETTING;
    use crate::lsp::harness::LspHarness;
    use crate::lsp::main_loop::DiagnosticsPublication;
    use crate::lsp::main_loop::Event;
    use crate::lsp::traits::url::UrlExt;

    /// Bounds every wait so a lost wakeup fails here rather than hanging to the
    /// harness timeout.
    const TIMEOUT: Duration = Duration::from_secs(10);

    async fn session() -> LspSession {
        LspHarness::new(OakDatabase::new()).start(&[]).await
    }

    fn publication(message: &str) -> DiagnosticsPublication {
        let url = url::Url::parse("file:///probe.R").unwrap();
        DiagnosticsPublication {
            path: FilePath::from_url(&url),
            uri: url.to_uri().unwrap(),
            diagnostics: vec![Diagnostic {
                message: String::from(message),
                ..Default::default()
            }],
            version: None,
        }
    }

    fn ready(generation: u64, message: &str) -> Event {
        Event::DiagnosticsReady(DiagnosticsReady {
            generation,
            publication: publication(message),
        })
    }

    /// Keep the pool busy after its only event is sent, until released.
    fn hold_pool(session: &LspSession) -> Sender<()> {
        let events_tx = session.events_tx();
        let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        session.spawn_analysis_task(move |_snapshot| {
            events_tx.send(ready(1, "from the task")).unwrap();
            blocked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        blocked_rx.recv_timeout(TIMEOUT).unwrap();
        release_tx
    }

    /// The task's event is handled while the task is still running, so no
    /// event arrives when the pool later goes idle. Settlement has to notice
    /// that through its periodic re-check.
    #[tokio::test]
    async fn test_settle_wakes_when_work_finishes_after_the_wait() {
        let mut session = session().await;
        let release = hold_pool(&session);

        let mut settling = Box::pin(session.settle());
        assert!(futures::poll!(&mut settling).is_pending());

        release.send(()).unwrap();
        tokio::time::timeout(TIMEOUT, settling).await.unwrap();
    }

    /// Rejecting an unsupported request lets the server handler finish before
    /// settlement reports the request on the test thread.
    #[tokio::test]
    #[should_panic(expected = "does not handle: [\"workspace/workspaceFolders\"]")]
    async fn test_settle_reports_an_unexpected_client_request() {
        let mut session = session().await;

        assert!(session.client.client().workspace_folders().await.is_err());

        tokio::time::timeout(TIMEOUT, session.settle())
            .await
            .unwrap();
    }

    /// Settings handed to [`LspHarness::start()`] are useless unless the
    /// session also runs the production pull, which needs the capabilities
    /// from the `initialize` request and the `initialized` handler.
    #[tokio::test]
    async fn test_start_pulls_the_supplied_settings() {
        let session = LspHarness::new(OakDatabase::new())
            .start(&[(R_DIAGNOSTICS_ENABLED_SETTING, Value::Bool(false))])
            .await;

        assert!(session
            .client
            .answered_requests()
            .contains(&Ok(String::from("workspace/configuration"))));
        assert!(!session.world().config.diagnostics.enable);
    }

    #[tokio::test]
    async fn test_source_fetching_bypass_survives_the_settings_pull() {
        let session = LspHarness::with_default_source_fetching(OakDatabase::new())
            .start(&[(OAK_SOURCE_FETCHING_ENABLED_SETTING, Value::Bool(false))])
            .await;

        assert!(session.world().ignore_source_fetching_env);
        assert!(!session.world().config.oak.source_fetching_enabled);
    }

    /// Collection moves an answer out of the pending list, and it stays
    /// readable after later events are handled.
    #[tokio::test]
    async fn test_definition_answer_stays_readable_after_collection() {
        let workspace = tempfile::tempdir().unwrap();
        let script = workspace.path().join("script.R");
        let mut session = session().await;

        session.send_did_open(&script, "f <- function() 1\nf()\n");
        let request = session.send_goto_definition(&script, Position::new(1, 0));
        tokio::time::timeout(TIMEOUT, session.settle())
            .await
            .unwrap();

        session.send_did_change(&script, "f <- function() 2\nf()\n", 1);
        tokio::time::timeout(TIMEOUT, session.settle())
            .await
            .unwrap();

        assert!(session.pending_definitions.is_empty());
        let Some(answer) = session.definition_answer(request) else {
            panic!("The definition request was never answered");
        };
        assert!(matches!(answer.response, Ok(Some(_))));
    }

    /// Handles are indices into the sending session's answers, so another
    /// session must reject them rather than read an unrelated answer at the
    /// same index.
    #[tokio::test]
    #[should_panic(expected = "was sent by another session")]
    async fn test_definition_answer_rejects_another_sessions_request() {
        let workspace = tempfile::tempdir().unwrap();
        let script = workspace.path().join("script.R");

        let mut first = session().await;
        let request = first.send_goto_definition(&script, Position::new(0, 0));
        drop(first);

        let mut second = session().await;
        second.send_goto_definition(&script, Position::new(0, 0));
        second.definition_answer(request);
    }

    /// The queued notifications and the wait cover one complete round trip
    /// through the event loop, diagnostics scheduler, and analysis pool.
    #[tokio::test]
    async fn test_wait_returns_the_publication_for_a_queued_change() {
        let workspace = tempfile::tempdir().unwrap();
        let script = workspace.path().join("script.R");
        let mut session = session().await;

        session.send_did_open(&script, "x <- 1\n");
        let opened =
            tokio::time::timeout(TIMEOUT, session.wait_for_accepted_diagnostics(&script, 0))
                .await
                .unwrap();
        assert_eq!(opened, vec![]);

        session.send_did_change(&script, "y <- 2\n", 1);
        let changed =
            tokio::time::timeout(TIMEOUT, session.wait_for_accepted_diagnostics(&script, 1))
                .await
                .unwrap();

        assert_eq!(changed, vec![]);
        assert_eq!(session.raw_publications().len(), 2);
    }

    /// Stacking changes without settling exposes intermediate versions to keyed
    /// replacement, so only the final version is guaranteed to publish.
    #[tokio::test]
    async fn test_wait_settles_a_burst_on_its_final_version() {
        let workspace = tempfile::tempdir().unwrap();
        let script = workspace.path().join("script.R");
        let mut session = session().await;

        session.send_did_open(&script, "x <- 1\n");

        for version in 1..=5 {
            session.send_did_change(&script, &format!("x <- {version}\n"), version);
        }

        let diagnostics =
            tokio::time::timeout(TIMEOUT, session.wait_for_accepted_diagnostics(&script, 5))
                .await
                .unwrap();

        assert_eq!(diagnostics, vec![]);

        // Acceptance is monotonic in generation, so draining what the burst
        // left behind cannot publish an older version over the one waited on.
        tokio::time::timeout(TIMEOUT, session.settle())
            .await
            .unwrap();
        assert_eq!(session.raw_publications().last().unwrap().version, Some(5));
    }
}
