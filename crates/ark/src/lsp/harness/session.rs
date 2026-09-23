//! Event-driven LSP harness support.
//!
//! [`LspSession`] consumes an [`LspHarness`] and drives production event handling
//! against a simulated editor without rebuilding its database. Editor operations
//! bypass incoming JSON-RPC parsing, while server-to-editor traffic uses the
//! production client path.
//!
//! Only one session may exist per process because the auxiliary sender is
//! process-global.

use std::path::Path;
use std::sync::Arc;

use aether_path::FilePath;
use serde_json::Value;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::Uri;

use super::client::TestClient;
use super::events;
use super::LspHarness;
use crate::lsp::analysis::PoolMetrics;
#[cfg(test)]
use crate::lsp::analysis::WorldStateSnapshot;
use crate::lsp::backend::LspResponse;
use crate::lsp::backend::RequestResponse;
use crate::lsp::main_loop::register_auxiliary_tx;
use crate::lsp::main_loop::AuxiliaryEvent;
use crate::lsp::main_loop::AuxiliaryState;
use crate::lsp::main_loop::DiagnosticsPublication;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::TokioUnboundedReceiver;
#[cfg(test)]
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceScheduler;
#[cfg(test)]
use crate::lsp::state::WorldState;
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

    #[cfg(test)]
    pub(crate) async fn start_with_sources(
        self,
        settings: &[(&str, Value)],
        source_handler: Arc<dyn SourceHandler>,
    ) -> LspSession {
        self.start_with(settings, Some(source_handler)).await
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
            auxiliary: AuxiliaryState::new(client.client()),
            state,
            client,
            auxiliary_rx,
            raw_publications: Vec::new(),
            delivered: 0,
            background_panics: 0,
        };

        session.handshake(folders).await;
        session
    }
}

/// Keeps each selected result owned while [`LspSession::pump_once()`] borrows
/// several fields. Keep [`Event`] inline because boxing it would add an
/// allocation to every main-loop event.
#[expect(clippy::large_enum_variant)]
enum Wakeup {
    Event(Event),
    Auxiliary(AuxiliaryEvent),
    Idle,
}

/// A production main loop connected to a simulated editor.
pub struct LspSession {
    state: GlobalState,
    client: TestClient,

    auxiliary: AuxiliaryState,
    auxiliary_rx: TokioUnboundedReceiver<AuxiliaryEvent>,

    /// Main-loop publications before unchanged diagnostics are suppressed.
    raw_publications: Vec<DiagnosticsPublication>,

    /// Number of raw publications already passed to the auxiliary handler.
    /// Replaying one after a later change can re-notify the client or restore
    /// diagnostics that a later publication cleared.
    delivered: usize,

    background_panics: usize,
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
        let (event, mut response_rx) = events::initialize(folders);
        self.state.handle_event_once(event).await;
        self.collect_auxiliary();

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

        self.state.handle_event_once(events::initialized()).await;
        self.collect_auxiliary();

        self.settle().await;
    }

    /// Queue `didOpen` for the main loop to pick up, the way an editor does.
    /// Unlike [`LspHarness::prepare_document()`], which only registers the
    /// buffer, this runs the open handler and the diagnostics it schedules.
    pub fn send_did_open(&self, path: &Path, contents: &str) {
        self.enqueue(events::did_open(path, contents));
    }

    /// Queue a whole-document `didChange` at `version`, which must exceed the
    /// version the document was opened at.
    ///
    /// Queueing rather than handling inline keeps main-loop dispatch inside a
    /// measured section, and lets a caller stack several changes to exercise
    /// keyed replacement.
    pub fn send_did_change(&self, path: &Path, contents: &str, version: i32) {
        self.enqueue(events::did_change(path, contents, version));
    }

    /// Queue an event without handling it, to model a busy main loop.
    pub(crate) fn enqueue(&self, event: Event) {
        self.state.events_tx().send(event).unwrap();
    }

    /// Pump the main loop until it accepts diagnostics for `path` at
    /// `version`, and hand them back.
    ///
    /// This covers the full round trip through the event loop, the diagnostics
    /// scheduler, the analysis pool, and generation filtering, while leaving
    /// out the serialization and socket work that
    /// [`Self::wait_for_diagnostics()`] adds. Publications already recorded
    /// when the wait starts don't count, so an earlier round trip over the
    /// same document can't satisfy it.
    ///
    /// A settled loop has no work left that could produce the publication, so
    /// reaching that state without it is a failure rather than a longer wait.
    pub async fn wait_for_accepted_diagnostics(
        &mut self,
        path: &Path,
        version: i32,
    ) -> Vec<Diagnostic> {
        let Some(path) = FilePath::from_path_buf(path.to_path_buf()) else {
            panic!("Not an absolute UTF-8 path: {}", path.display());
        };
        let first = self.raw_publications.len();

        loop {
            let accepted = self.raw_publications[first..].iter().find(|publication| {
                publication.path == path && publication.version == Some(version)
            });
            if let Some(accepted) = accepted {
                return accepted.diagnostics.clone();
            }

            if self.state.is_settled() {
                panic!(
                    "The main loop settled without accepting diagnostics for {path:?} at \
                     version {version}"
                );
            }

            self.pump_once().await;
        }
    }

    /// Process queued events until no scheduler or analysis work remains.
    ///
    /// Settlement also fails on panics, unbalanced pool counters, or requests
    /// unsupported by the simulated editor.
    pub async fn settle(&mut self) {
        while !self.state.is_settled() {
            self.pump_once().await;
        }
        self.check_background_failures();
    }

    /// Report whether the loop has no scheduler, analysis, or queued work
    /// left, so a caller can assert an idle starting point before measuring.
    pub fn is_settled(&self) -> bool {
        self.state.is_settled()
    }

    /// Wait for the next main-loop or auxiliary wakeup and handle it.
    ///
    /// The idle signal wakes this when a worker records its terminal outcome
    /// after sending its result. A retained permit covers a transition that
    /// races the caller's settled check.
    async fn pump_once(&mut self) {
        // Limit the select's borrows so handling can reborrow `self`.
        let wakeup = {
            let idle = self.state.analysis_idle_signal();
            tokio::select! {
                event = self.state.next_event() => Wakeup::Event(event),
                Some(event) = self.auxiliary_rx.recv() => Wakeup::Auxiliary(event),
                _ = idle.notified() => Wakeup::Idle,
            }
        };

        match wakeup {
            Wakeup::Event(event) => {
                self.state.handle_event_once(event).await;
                self.collect_auxiliary();
            },
            Wakeup::Auxiliary(event) => self.record_auxiliary(event),
            Wakeup::Idle => {},
        }

        // A panicking worker can leave its scheduler pending, preventing a
        // final settled check. Inspect failures after every wakeup.
        self.check_background_failures();
    }

    /// Settle and return diagnostics received by the client for `uri`.
    ///
    /// Unchanged diagnostics are suppressed by production deduplication, so a
    /// notification count cannot signal completion. Use
    /// [`Self::wait_for_accepted_diagnostics()`] to observe main-loop output
    /// instead.
    pub async fn wait_for_diagnostics(&mut self, uri: &Uri) -> Option<Vec<Value>> {
        self.settle().await;
        self.deliver_auxiliary().await;

        // The socket is FIFO, so a round-trip flushes the notifications the
        // peer has not read yet.
        let _ = self.client.client().configuration(vec![]).await;

        self.client
            .notifications()
            .into_iter()
            .rfind(|(method, params)| {
                method == "textDocument/publishDiagnostics" &&
                    params["uri"].as_str() == Some(uri.as_str())
            })
            .map(|(_method, params)| match &params["diagnostics"] {
                Value::Array(diagnostics) => diagnostics.clone(),
                _ => Vec::new(),
            })
    }

    pub fn client_notifications(&self) -> Vec<(String, Value)> {
        self.client.notifications()
    }

    pub fn analysis_metrics(&self) -> PoolMetrics {
        self.state.lsp_state().analysis_pool.metrics()
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

    async fn deliver_auxiliary(&mut self) {
        self.collect_auxiliary();

        while self.delivered < self.raw_publications.len() {
            let publication = self.raw_publications[self.delivered].clone();
            self.auxiliary.publish_diagnostics(publication).await;
            self.delivered += 1;
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

#[cfg(test)]
impl LspSession {
    /// Handle one event without settling its follow-up work.
    pub(crate) async fn handle_once(&mut self, event: Event) {
        self.state.handle_event_once(event).await;
        self.collect_auxiliary();
    }

    pub(crate) async fn pump_scans_to_quiescence(&mut self) {
        self.state.pump_scans_to_quiescence().await;
        self.collect_auxiliary();
    }

    /// Handle `didOpen` inline, so the caller observes the state the open
    /// handler leaves behind before any follow-up work runs.
    pub(crate) async fn open_document(&mut self, path: &Path, contents: &str) {
        self.handle_once(events::did_open(path, contents)).await;
    }

    /// Handle a whole-document `didChange` at `version` inline, as
    /// [`Self::open_document()`] does for `didOpen`.
    pub(crate) async fn change_document(&mut self, path: &Path, contents: &str, version: i32) {
        self.handle_once(events::did_change(path, contents, version))
            .await;
    }

    pub(crate) fn world(&self) -> &WorldState {
        self.state.world()
    }

    pub(crate) fn raw_publications(&self) -> &[DiagnosticsPublication] {
        &self.raw_publications
    }

    pub(crate) fn events_tx(&self) -> TokioUnboundedSender<Event> {
        self.state.events_tx()
    }

    /// Run `task` on the analysis pool without going through a scheduler. Tests
    /// use this to create worker states that normal requests cannot hold, such
    /// as sending an [`Event`] and then blocking before the task returns.
    pub(crate) fn spawn_analysis_task(
        &self,
        task: impl FnOnce(WorldStateSnapshot) + Send + 'static,
    ) {
        self.state
            .lsp_state()
            .analysis_pool
            .spawn_test_task(self.state.world().snapshot(), task);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;
    use std::sync::mpsc::Sender;
    use std::time::Duration;
    use std::time::Instant;

    use aether_path::FilePath;
    use oak_db::OakDatabase;
    use serde_json::Value;
    use tower_lsp_server::ls_types::Diagnostic;
    use tower_lsp_server::ls_types::Uri;

    use super::LspSession;
    use crate::lsp::analysis::DiagnosticsReady;
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

    fn cleared(generation: u64) -> Event {
        let mut publication = publication("");
        publication.diagnostics.clear();
        Event::DiagnosticsReady(DiagnosticsReady {
            generation,
            publication,
        })
    }

    fn ready(generation: u64, message: &str) -> Event {
        Event::DiagnosticsReady(DiagnosticsReady {
            generation,
            publication: publication(message),
        })
    }

    /// Send a result before the task records its terminal outcome, then return
    /// the gate that controls that outcome.
    fn spawn_gated_analysis_task(session: &LspSession) -> (Receiver<()>, Sender<()>) {
        let events_tx = session.events_tx();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        session.spawn_analysis_task(move |_snapshot| {
            events_tx.send(ready(1, "from the task")).unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        entered_rx.recv_timeout(TIMEOUT).unwrap();
        (entered_rx, release_tx)
    }

    /// Wait until the worker records its terminal outcome. This establishes a
    /// precondition, not an ordering guarantee.
    async fn await_recorded_terminal(session: &LspSession) {
        let deadline = Instant::now() + TIMEOUT;
        while session.analysis_metrics().running() != 0 {
            assert!(Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
    }

    /// Settlement must check the pool before waiting because this worker became
    /// idle before `settle()` started. The retained permit covers the separate
    /// check-to-wait race by construction.
    #[tokio::test]
    async fn test_settle_returns_when_work_finished_before_the_wait() {
        let mut session = session().await;
        let (_entered, release) = spawn_gated_analysis_task(&session);

        release.send(()).unwrap();
        await_recorded_terminal(&session).await;

        tokio::time::timeout(TIMEOUT, session.settle())
            .await
            .unwrap();

        assert_eq!(session.analysis_metrics().completed, 1);
    }

    #[tokio::test]
    async fn test_settle_wakes_when_work_finishes_after_the_wait() {
        let mut session = session().await;
        let (_entered, release) = spawn_gated_analysis_task(&session);

        let mut settling = Box::pin(session.settle());
        assert!(futures::poll!(&mut settling).is_pending());

        release.send(()).unwrap();
        tokio::time::timeout(TIMEOUT, settling).await.unwrap();
    }

    /// Identical diagnostics produce two raw publications but one client
    /// notification after deduplication. Completion tests must therefore inspect
    /// [`LspSession::raw_publications`], not notification counts.
    #[tokio::test]
    async fn test_raw_publications_outnumber_client_notifications() {
        let mut session = session().await;

        session.enqueue(ready(1, "same every pass"));
        session.enqueue(ready(2, "same every pass"));

        let uri: Uri = "file:///probe.R".parse().unwrap();
        let delivered = tokio::time::timeout(TIMEOUT, session.wait_for_diagnostics(&uri))
            .await
            .unwrap();

        assert_eq!(session.raw_publications().len(), 2);
        assert_eq!(delivered.map(|diagnostics| diagnostics.len()), Some(1));

        let published: Vec<String> = session
            .client_notifications()
            .into_iter()
            .map(|(method, _params)| method)
            .collect();
        assert_eq!(published, vec![String::from(
            "textDocument/publishDiagnostics"
        )]);
    }

    /// Replaying previously delivered diagnostics can restore a set that a
    /// later publication cleared, so repeated waits must deliver nothing new.
    #[tokio::test]
    async fn test_repeated_waits_deliver_nothing_new() {
        let mut session = session().await;
        let uri: Uri = "file:///probe.R".parse().unwrap();

        session.enqueue(ready(1, "first"));
        session.enqueue(ready(2, "second"));
        session.enqueue(cleared(3));

        tokio::time::timeout(TIMEOUT, session.wait_for_diagnostics(&uri))
            .await
            .unwrap();
        assert_eq!(session.client_notifications().len(), 3);

        let delivered = tokio::time::timeout(TIMEOUT, session.wait_for_diagnostics(&uri))
            .await
            .unwrap();

        assert_eq!(session.client_notifications().len(), 3);
        assert_eq!(delivered, Some(vec![]));
        assert_eq!(session.raw_publications().len(), 3);
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
