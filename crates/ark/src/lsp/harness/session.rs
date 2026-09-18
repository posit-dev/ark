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

use serde_json::Value;
use tower_lsp_server::ls_types::Uri;

use super::LspHarness;
use crate::lsp::analysis::PoolMetrics;
use crate::lsp::analysis::WorldStateSnapshot;
use crate::lsp::main_loop::register_auxiliary_tx;
use crate::lsp::main_loop::AuxiliaryEvent;
use crate::lsp::main_loop::AuxiliaryState;
use crate::lsp::main_loop::DiagnosticsPublication;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::TokioUnboundedReceiver;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceScheduler;
use crate::lsp::tests::utils::client::TestClient;
use crate::lsp::tests::utils::events;

impl LspHarness {
    /// The simulated editor serves `settings` for `workspace/configuration`, and
    /// `None` disables source fetching. Consuming the harness prevents buffers
    /// from being prepared outside the main loop after the session starts.
    pub(crate) async fn start(
        self,
        settings: &[(&str, Value)],
        source_handler: Option<Arc<dyn SourceHandler>>,
    ) -> LspSession {
        let client = TestClient::new(settings).await;

        // Register before constructing the session because the main loop may
        // publish on its first tick.
        let (auxiliary_tx, auxiliary_rx) = tokio::sync::mpsc::unbounded_channel();
        register_auxiliary_tx(auxiliary_tx);

        let mut source_scheduler = SourceScheduler::new(source_handler);
        source_scheduler.config_arrived();

        let state = GlobalState::from_parts(
            client.client(),
            self.state,
            LspState::new(tokio::sync::mpsc::unbounded_channel().0, source_scheduler),
        );

        LspSession {
            auxiliary: AuxiliaryState::new(client.client()),
            state,
            client,
            auxiliary_rx,
            raw_publications: Vec::new(),
            delivered: 0,
            background_panics: 0,
        }
    }
}

/// Keeps each selected result owned while [`LspSession::settle()`] borrows
/// several fields. [`Event`] already dominates the enum's size, so boxing would
/// add an allocation to every loop iteration.
#[expect(clippy::large_enum_variant)]
enum Wakeup {
    Event(Event),
    Auxiliary(AuxiliaryEvent),
    Idle,
}

/// A production main loop connected to a simulated editor.
pub(crate) struct LspSession {
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
    /// Handle one event without settling follow-up work, for tests that gate a
    /// subsystem.
    pub(crate) async fn handle_once(&mut self, event: Event) {
        self.state.handle_event_once(event).await;
        self.collect_auxiliary();
    }

    /// Queue an event without handling it, to model a busy main loop.
    pub(crate) fn enqueue(&self, event: Event) {
        self.state.events_tx().send(event).unwrap();
    }

    pub(crate) async fn pump_scans_to_quiescence(&mut self) {
        self.state.pump_scans_to_quiescence().await;
        self.collect_auxiliary();
    }

    /// Deliver `didOpen`, unlike [`LspHarness::prepare_document`], which only
    /// registers the buffer.
    pub(crate) async fn open_document(&mut self, path: &Path, contents: &str) {
        self.handle_once(events::did_open(path, contents)).await;
    }

    /// Deliver a whole-document `didChange` at `version`, which must exceed the
    /// version the document was opened at.
    pub(crate) async fn change_document(&mut self, path: &Path, contents: &str, version: i32) {
        self.handle_once(events::did_change(path, contents, version))
            .await;
    }

    /// Process queued events until no scheduler or analysis work remains.
    ///
    /// The idle signal wakes this loop when a worker records its terminal
    /// outcome after sending its result. A retained permit covers a transition
    /// that races the settled check. Panics and unbalanced pool counters still
    /// fail settlement rather than masquerading as successful completion.
    pub(crate) async fn settle(&mut self) {
        while !self.state.is_settled() {
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

        self.check_background_failures();
    }

    /// Settle and return diagnostics received by the client for `uri`.
    ///
    /// Unchanged diagnostics are suppressed by production deduplication, so a
    /// notification count cannot signal completion. Use
    /// [`Self::raw_publications`] to inspect main-loop output instead.
    pub(crate) async fn wait_for_diagnostics(&mut self, uri: &Uri) -> Option<Vec<Value>> {
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

    pub(crate) fn raw_publications(&self) -> &[DiagnosticsPublication] {
        &self.raw_publications
    }

    pub(crate) fn client_notifications(&self) -> Vec<(String, Value)> {
        self.client.notifications()
    }

    pub(crate) fn events_tx(&self) -> TokioUnboundedSender<Event> {
        self.state.events_tx()
    }

    pub(crate) fn analysis_metrics(&self) -> PoolMetrics {
        self.state.lsp_state().analysis_pool.metrics()
    }

    pub(crate) fn spawn_analysis_probe(
        &self,
        run: impl FnOnce(WorldStateSnapshot) + Send + 'static,
    ) {
        self.state
            .lsp_state()
            .analysis_pool
            .spawn_probe(self.state.world().snapshot(), run);
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

        // Negative derived counters would make inconsistent bookkeeping appear
        // as an empty pool.
        if metrics.waiting() < 0 || metrics.running() < 0 {
            panic!("The analysis pool's counters are unbalanced: {metrics:?}");
        }
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
    use tower_lsp_server::ls_types::Diagnostic;
    use tower_lsp_server::ls_types::Uri;

    use super::LspSession;
    use crate::lsp::analysis::DiagnosticsReady;
    use crate::lsp::harness::LspHarness;
    use crate::lsp::main_loop::DiagnosticsPublication;
    use crate::lsp::main_loop::Event;
    use crate::lsp::traits::url::UrlExt;

    /// Bounds every wait so a lost wakeup fails here rather than hanging to the
    /// harness timeout.
    const TIMEOUT: Duration = Duration::from_secs(10);

    async fn session() -> LspSession {
        LspHarness::new(OakDatabase::new()).start(&[], None).await
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
    fn gated_probe(session: &LspSession) -> (Receiver<()>, Sender<()>) {
        let events_tx = session.events_tx();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        session.spawn_analysis_probe(move |_snapshot| {
            events_tx.send(ready(1, "from the probe")).unwrap();
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
        let (_entered, release) = gated_probe(&session);

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
        let (_entered, release) = gated_probe(&session);

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
}
