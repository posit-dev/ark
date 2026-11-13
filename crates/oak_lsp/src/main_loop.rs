//
// main_loop.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::collections::HashSet;
use std::mem::discriminant;
use std::mem::Discriminant;
use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;

use aether_path::FilePath;
use anyhow::anyhow;
use oak_db::Environment;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::SourceDb;
use oak_scan::DbScan;
use oak_scan::ScanCompleted;
use oak_scan::ScanRequest;
use oak_scan::ScanScheduler;
use stdext::result::ResultExt;
use stdext::spawn;
use tokio::runtime::Handle;
use tokio::sync::mpsc::unbounded_channel as tokio_unbounded_channel;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tower_lsp_server::jsonrpc;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::request;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::MessageType;
use tower_lsp_server::ls_types::ShowMessageRequestParams;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::Client;

use super::backend::RequestResponse;
use crate::analysis;
use crate::analysis::catch_cancellation;
use crate::analysis::AnalysisPool;
use crate::analysis::DiagnosticsMetrics;
use crate::analysis::DiagnosticsReady;
use crate::analysis::DiagnosticsState;
use crate::backend::LspError;
use crate::backend::LspMessage;
use crate::backend::LspNotification;
use crate::backend::LspRequest;
use crate::backend::LspResponse;
use crate::backend::LspResult;
use crate::capabilities::Capabilities;
use crate::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;
use crate::environments::unresolved_database;
use crate::environments::EnvironmentCompleted;
use crate::environments::EnvironmentResolver;
use crate::environments::EnvironmentScheduler;
use crate::environments::RProcessResolver;
use crate::handlers;
use crate::handlers::EventContext;
use crate::handlers::Handlers;
use crate::handlers::RequestContext;
use crate::io_pool::IoPool;
use crate::panic;
use crate::panic::Recovery;
use crate::sidecar::OakSidecarHandler;
use crate::sidecar::SidecarCompleted;
use crate::sidecar::SidecarHandler;
use crate::sidecar::SidecarScheduler;
use crate::sources::source_fetching_disabled_by_ci;
use crate::sources::OakSourceHandler;
use crate::sources::SourceCompleted;
use crate::sources::SourceHandler;
use crate::sources::SourceOrigin;
use crate::sources::SourceResponse;
use crate::sources::SourceScheduler;
use crate::state::WorldState;
use crate::state_handlers;
use crate::state_handlers::LibraryWatcherRegistration;
use crate::traits::url::UriExt;
use crate::watchdog::Watchdog;

pub type TokioUnboundedSender<T> = tokio::sync::mpsc::UnboundedSender<T>;
pub(crate) type TokioUnboundedReceiver<T> = tokio::sync::mpsc::UnboundedReceiver<T>;

/// The global instance of the auxiliary event channel, used for sending log messages or
/// spawning threads from free functions. Since this is an unbounded channel, sending a
/// log message is not async nor blocking. Tokio senders are Send and Sync so this global
/// variable can be safely shared across threads.
///
/// LSP sessions can be restarted or reconnected at any time, which is why this is an
/// `RwLock`, but we expect that to be very rare. Read locking is not expected to be
/// contentious.
///
/// Note that in case of duplicate LSP sessions (see
/// https://github.com/posit-dev/ark/issues/622 and
/// https://github.com/posit-dev/positron/issues/5321), it's possible for older
/// LSPs to send log messages and tasks to the newer LSPs.
static AUXILIARY_EVENT_TX: RwLock<Option<TokioUnboundedSender<AuxiliaryEvent>>> = RwLock::new(None);

/// State shared across one LSP service.
#[derive(Debug)]
pub(crate) struct LspServiceContext {
    crashed: AtomicBool,
    background_panic_reported: AtomicBool,
}

impl LspServiceContext {
    pub(crate) fn new() -> Self {
        Self {
            crashed: AtomicBool::new(false),
            background_panic_reported: AtomicBool::new(false),
        }
    }

    pub(crate) fn has_crashed(&self) -> bool {
        self.crashed.load(Ordering::Acquire)
    }

    pub(crate) fn set_crashed(&self) {
        self.crashed.store(true, Ordering::Release);
    }

    pub(crate) fn report_background_panic(&self) {
        if self.background_panic_reported.swap(true, Ordering::AcqRel) {
            return;
        }

        report_background_panic();
    }
}

#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
pub enum Event<H: Handlers> {
    Lsp(LspMessage<H>),
    /// An event the host posts, handled by [`Handlers::event()`].
    Host(H::Event),
    OakScanCompleted(ScanCompleted),
    OakScanFailed(Root),
    SourceCompleted(SourceCompleted),
    SidecarCompleted(SidecarCompleted),
    EnvironmentResolved(EnvironmentCompleted),
    DiagnosticsReady(DiagnosticsReady),
    #[cfg(feature = "testing")]
    TestPanicMainLoop,
}

impl<H: Handlers> Event<H> {
    /// Label for the `handle_event` tracing span. Coarse on purpose: LSP
    /// handlers are instrumented and name themselves as child spans, and the
    /// variants' payloads can carry document contents.
    fn kind(&self) -> &'static str {
        match self {
            Event::Lsp(_) => "lsp",
            Event::Host(_) => "host",
            Event::OakScanCompleted(_) => "oak_scan_completed",
            Event::OakScanFailed(_) => "oak_scan_failed",
            Event::SourceCompleted(_) => "source_completed",
            Event::SidecarCompleted(_) => "sidecar_completed",
            Event::EnvironmentResolved(_) => "environment_resolved",
            Event::DiagnosticsReady(_) => "diagnostics_ready",
            #[cfg(feature = "testing")]
            Event::TestPanicMainLoop => "test_panic_main_loop",
        }
    }
}

#[derive(Debug)]
pub enum AuxiliaryEvent {
    Log(lsp_types::MessageType, String),
    PublishDiagnostics(DiagnosticsPublication),
    ReportBackgroundPanic,
    Shutdown,
    #[cfg(feature = "testing")]
    TestPanic,
}

/// Global state for the main loop
///
/// This is a singleton that fully owns the source of truth for `WorldState`
/// which contains the inputs of all LSP methods. The `main_loop()` method is
/// the heart of the LSP. The tower-lsp backend and the host communicate with
/// the main loop through the `Event` channel that is passed on construction.
/// The host answers requests and handles its own events through `H`.
pub struct GlobalState<H: Handlers> {
    /// The global world state containing all inputs for LSP analysis lives
    /// here. The dispatcher provides refs, exclusive refs, or snapshots to
    /// handlers.
    world: WorldState,

    /// The non-cloneable, per-session LSP state. Only used in exclusive ref
    /// handlers.
    lsp_state: LspState,

    /// The host's request and event handlers.
    handlers: H,

    /// LSP client shared with tower-lsp and the log loop
    client: Client,

    /// Request handlers whose panic has already been reported to the user.
    reported_request_panics: HashSet<RequestKind<H>>,

    /// Event channels for the main loop. The tower-lsp methods forward
    /// notifications and requests here via `Event::Lsp`. The host posts its
    /// events via `Event::Host`, and background jobs send their results back
    /// through the other variants.
    events_tx: TokioUnboundedSender<Event<H>>,
    events_rx: TokioUnboundedReceiver<Event<H>>,

    /// Diagnostics need a refresh whenever the current oak revision differs.
    last_refreshed_revision: salsa::plumbing::Revision,

    /// Preserved across intervening writes so a steady backlog cannot reset
    /// the refresh deadline.
    refresh_deferred_since: Option<std::time::Instant>,
}

/// Refresh diagnostics after this delay even if more events are queued.
/// The check runs between event handlers, so a long-running handler can delay
/// the refresh further. This does not bound diagnostics completion time.
const MAX_REFRESH_DELAY: Duration = Duration::from_millis(50);

/// Owns the running LSP loops. Dropping it shuts them down.
///
/// - The auxiliary loop is a runtime task, aborted when `_aux_loop` drops.
/// - The main loop runs on its own thread. Dropping `_main_shutdown_tx` closes
///   the channel the loop selects on, so it breaks, drops the owned
///   `GlobalState`, and the thread exits. We hold the thread's handle but don't
///   join it on drop (it winds down on its own), so dropping never blocks the
///   caller.
#[derive(Debug)]
pub(crate) struct LoopHandles {
    _main_loop: std::thread::JoinHandle<()>,
    _main_shutdown_tx: oneshot::Sender<()>,
    _aux_loop: tokio::task::JoinSet<()>,
}

/// Workers on [`LspState::source_pool`]. We never have more than this many
/// package downloads in flight.
pub(crate) const SOURCE_POOL_THREADS: usize = 2;

/// Non-cloneable, per-session state mutated only by exclusive handlers.
/// Sits alongside [`WorldState`], which the main loop owns. State that can't
/// travel with a snapshot lives here instead.
pub struct LspState {
    /// Capabilities negotiated with the client
    pub(crate) capabilities: Capabilities,

    /// Coordinator for asynchronous workspace scans. Mutated only from
    /// main-loop handlers. Must be out of [`WorldState`] because the scheduler
    /// is not clonable.
    pub(crate) oak_scheduler: ScanScheduler,

    /// Scheduler of [crate::sources::SourceRequest]s. Scheduling and source
    /// consumption all happen from the main loop.
    pub(crate) source_scheduler: SourceScheduler,

    /// Sidecar scheduling and response handling stay on the main loop.
    pub sidecar_scheduler: SidecarScheduler,

    /// Keeps resolution policy and result application on the main loop. The
    /// background jobs hold no database handle.
    pub(crate) environment_scheduler: EnvironmentScheduler,

    /// The client's registration of the library watchers, replaced when the
    /// merged library list changes.
    pub(crate) library_watchers: LibraryWatcherRegistration,

    /// Threads running diagnostics and index warmup. The only executor a db
    /// snapshot is allowed on, see [`crate::analysis`].
    pub(crate) analysis_pool: AnalysisPool,

    /// Per-file generation bookkeeping the main loop uses to drop a
    /// diagnostics result superseded by a newer refresh. See
    /// [`crate::analysis::DiagnosticsState`].
    pub(crate) diagnostics: DiagnosticsState,

    /// Session counters for diagnostics scheduling, separate so observability
    /// cannot affect the main loop's idle check.
    pub(crate) diagnostics_metrics: DiagnosticsMetrics,

    /// Threads running workspace scans.
    pub(crate) scan_pool: IoPool,

    /// Threads running package source fetches. Separate from [`Self::scan_pool`]
    /// so a startup scan never queues behind a download.
    pub(crate) source_pool: IoPool,

    /// Thread running the sidecar job. Separate from [`Self::source_pool`] so
    /// the job never queues behind a download.
    pub(crate) sidecar_pool: IoPool,

    /// Runs R startups with [`crate::environments::RESOLUTION_TIMEOUT`],
    /// then lists their libraries. Separate from [`Self::scan_pool`] so scans
    /// do not queue behind R startup, and from [`Self::source_pool`] so
    /// resolution does not queue behind downloads that depend on its results.
    pub(crate) environment_pool: IoPool,

    /// Detects a tick that arms and never disarms, usually a write parked
    /// behind a background task that can't drop its db snapshot. See
    /// [`crate::watchdog`].
    pub(crate) watchdog: Watchdog,

    /// State shared with the backend, main loop, and background pools.
    service_context: Arc<LspServiceContext>,
}

impl LspState {
    /// No scheduler has a handler or resolver, so no source fetch, sidecar
    /// job or environment resolution runs until
    /// [`Self::with_source_scheduler()`], [`Self::with_sidecar_scheduler()`]
    /// or [`Self::with_environment_scheduler()`] installs one.
    #[expect(
        clippy::new_without_default,
        reason = "`new()` spawns the analysis and I/O pool threads"
    )]
    pub fn new() -> Self {
        let service_context = Arc::new(LspServiceContext::new());

        Self {
            capabilities: Capabilities::default(),
            oak_scheduler: ScanScheduler::new(),
            source_scheduler: SourceScheduler::new(None),
            sidecar_scheduler: SidecarScheduler::new(None),
            environment_scheduler: EnvironmentScheduler::new(None),
            library_watchers: LibraryWatcherRegistration::default(),
            analysis_pool: AnalysisPool::new(Arc::clone(&service_context)),
            diagnostics: DiagnosticsState::default(),
            diagnostics_metrics: DiagnosticsMetrics::default(),
            // Stack size: `ScanRequest::run()` walks the filesystem with
            // `ignore::Walk` and `WalkDir`, both iterative, and parses
            // DESCRIPTION line by line.
            scan_pool: IoPool::new(
                "oak-scan",
                1,
                stdext::SMALL_STACK_SIZE,
                Arc::clone(&service_context),
            ),
            // Full stack because a fetch runs a rustls handshake, zstd and tar
            // decoding, and an R subprocess.
            source_pool: IoPool::new(
                "oak-source",
                SOURCE_POOL_THREADS,
                stdext::DEFAULT_STACK_SIZE,
                Arc::clone(&service_context),
            ),
            sidecar_pool: IoPool::new(
                "oak-sidecar",
                1,
                stdext::DEFAULT_STACK_SIZE,
                Arc::clone(&service_context),
            ),
            // A small stack suffices for waiting on R and listing direct library
            // entries with the iterative `WalkDir` walker.
            environment_pool: IoPool::new(
                "oak-env",
                1,
                stdext::SMALL_STACK_SIZE,
                Arc::clone(&service_context),
            ),
            watchdog: Watchdog::new(),
            service_context,
        }
    }

    pub fn with_source_scheduler(mut self, source_scheduler: SourceScheduler) -> Self {
        self.source_scheduler = source_scheduler;
        self
    }

    pub fn with_sidecar_scheduler(mut self, sidecar_scheduler: SidecarScheduler) -> Self {
        self.sidecar_scheduler = sidecar_scheduler;
        self
    }

    pub fn with_environment_scheduler(
        mut self,
        environment_scheduler: EnvironmentScheduler,
    ) -> Self {
        self.environment_scheduler = environment_scheduler;
        self
    }

    /// The schedulers' answers about pending work, for request handlers.
    pub(crate) fn readiness<'a>(&'a self, world: &'a WorldState) -> Readiness<'a> {
        Readiness {
            world,
            environment_scheduler: &self.environment_scheduler,
            source_scheduler: &self.source_scheduler,
            sidecar_scheduler: &self.sidecar_scheduler,
        }
    }
}

/// Reports whether library resolution, source fetching or sidecar jobs can
/// still change a request's answer. Handlers can use this to mark an answer
/// incomplete, but cannot schedule work through this interface.
pub struct Readiness<'a> {
    world: &'a WorldState,
    environment_scheduler: &'a EnvironmentScheduler,
    source_scheduler: &'a SourceScheduler,
    sidecar_scheduler: &'a SidecarScheduler,
}

impl Readiness<'_> {
    /// Whether library resolution is pending for `environment`, so its set of
    /// available packages can still change.
    pub fn libraries_pending(&self, environment: Environment) -> bool {
        self.environment_scheduler.is_pending(environment)
    }

    /// Whether neither the source fetch nor the sidecar job can still change
    /// `package`. A `pkg::` completion list stays incomplete until its package
    /// settles, since either job can add names or determine item kinds.
    pub fn package_settled(&self, package: Package) -> bool {
        let db = self.world.db();
        !self
            .source_scheduler
            .sources_may_arrive(db, &self.world.config.oak, package) &&
            !self.sidecar_scheduler.may_change(db, package)
    }
}

/// State for the auxiliary loop
///
/// The auxiliary loop handles latency-sensitive events such as log messages. A
/// main loop tick might takes many milliseconds and might have a lot of events
/// in queue, so it's not appropriate for events that need immediate handling.
///
/// The auxiliary loop currently handles:
/// - Log messages.
/// - Diagnostics publication.
/// - Background task panic reports.
struct AuxiliaryState {
    client: Client,
    /// Last non-empty diagnostics published per file. A refresh re-runs every
    /// open file, but most runs produce the same result, so we skip the publish
    /// when it matches what the client already has.
    published_diagnostics: HashMap<FilePath, Vec<Diagnostic>>,
}

impl<H: Handlers> GlobalState<H> {
    /// Create a new global state
    ///
    /// # Arguments
    ///
    /// * `handlers` supplies the host's request and event handlers.
    /// * `client`: The tower-lsp client shared with the tower-lsp backend
    ///   and auxiliary loop.
    /// * `r_executable` selects the R executable for source fetching, sidecar
    ///   jobs and library resolution. Without it, all three are disabled.
    pub(crate) fn new(handlers: H, client: Client, r_executable: Option<PathBuf>) -> Self {
        let source_handler = r_executable.as_deref().and_then(source_handler);
        let sidecar_handler = r_executable.as_deref().and_then(sidecar_handler);
        let resolver = r_executable
            .map(|r| Arc::new(RProcessResolver::new(r)) as Arc<dyn EnvironmentResolver>);

        Self::from_parts(
            handlers,
            client,
            WorldState::new(unresolved_database()),
            LspState::new()
                .with_source_scheduler(SourceScheduler::new(source_handler))
                .with_sidecar_scheduler(SidecarScheduler::new(sidecar_handler))
                .with_environment_scheduler(EnvironmentScheduler::new(resolver)),
        )
    }

    /// Assemble preconfigured [`WorldState`] and [`LspState`] values so tests
    /// can supply their own database, source handler, and environment resolver.
    pub fn from_parts(handlers: H, client: Client, world: WorldState, lsp_state: LspState) -> Self {
        // Transmission channel for the main loop events. Shared with the
        // tower-lsp backend and the host.
        let (events_tx, events_rx) = tokio_unbounded_channel::<Event<H>>();
        let last_refreshed_revision = world.revision();

        Self {
            world,
            lsp_state,
            handlers,
            client,
            reported_request_panics: HashSet::new(),
            events_tx,
            events_rx,
            last_refreshed_revision,
            refresh_deferred_since: None,
        }
    }

    /// Get `Event` transmission channel
    pub(crate) fn events_tx(&self) -> TokioUnboundedSender<Event<H>> {
        self.events_tx.clone()
    }

    pub(crate) fn service_context(&self) -> &Arc<LspServiceContext> {
        &self.lsp_state.service_context
    }

    /// Start the main and auxiliary loops.
    ///
    /// The returned [`LoopHandles`] owns everything the loops need. Drop it to
    /// shut the loops down and release the owned state.
    pub(crate) fn start(self, server_shutdown_tx: Sender<()>) -> LoopHandles {
        let mut aux = tokio::task::JoinSet::<()>::new();

        // The main loop can publish on its first tick, so register the
        // process-global auxiliary sender before starting it.
        let (auxiliary_event_tx, auxiliary_event_rx) = tokio_unbounded_channel::<AuxiliaryEvent>();
        register_auxiliary_tx(auxiliary_event_tx);

        let aux_state = AuxiliaryState::new(self.client.clone());
        aux.spawn(async move { aux_state.start(auxiliary_event_rx).await });

        // Since the main loop owns the Salsa DB and writes to it, we run on its
        // own thread instead of a Tokio worker. Salsa writes are potentially
        // blocking until the writer gains exclusive access. If background tasks
        // holding clones of the DB are stuck on the same thread as the main
        // loop, the LSP deadlocks. This can be avoided by wrapping writes in
        // `block_in_place()` but the safer structure is to have it run on an OS
        // thread that we're in control of.
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = Handle::current();
        let client = self.client.clone();
        let service_context = Arc::clone(&self.lsp_state.service_context);

        let main_loop = spawn!("oak-main-loop", move || {
            let outcome = panic::catch_unwind(Recovery::Always, {
                let server_shutdown_tx = server_shutdown_tx.clone();
                let handle = handle.clone();
                let service_context = Arc::clone(&service_context);
                move || {
                    handle.block_on(self.main_loop(
                        shutdown_rx,
                        server_shutdown_tx,
                        service_context,
                    ))
                }
            });

            // Handle panics that bypass `handle_event()`'s recovery boundary.
            if let Err(message) = outcome {
                crate::log_error!("Panic in the main loop: {message}");
                service_context.set_crashed();

                let report = panic::catch_unwind(Recovery::Always, || {
                    handle.block_on(report_crash(&client))
                });
                if let Err(message) = report {
                    log::error!("Panic while reporting an LSP crash: {message}");
                }

                // The runtime may be shutting down, so don't wait for channel capacity.
                server_shutdown_tx.try_send(()).log_err();
            }
        });

        LoopHandles {
            _main_shutdown_tx: shutdown_tx,
            _aux_loop: aux,
            _main_loop: main_loop,
        }
    }

    /// Run main loop
    ///
    /// This takes ownership of all global state and handles one by one LSP
    /// requests, notifications, and other internal events.
    async fn main_loop(
        mut self,
        mut shutdown_rx: oneshot::Receiver<()>,
        server_shutdown_tx: Sender<()>,
        service_context: Arc<LspServiceContext>,
    ) {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    crate::log_info!("Main loop stopping: handle dropped");
                    break;
                },
                event = self.events_rx.recv() => {
                    let Some(event) = event else {
                        crate::log_info!("Main loop stopping: event channel closed");
                        break;
                    };

                    #[cfg(feature = "testing")]
                    if matches!(event, Event::TestPanicMainLoop) {
                        panic!("Test panic outside the event recovery boundary");
                    }

                    match self.handle_event_in_boundary(event).await {
                        Ok((Ok(()), _span)) => {},
                        Ok((Err(err), _span)) => crate::log_error!("Failure while handling event:\n{err:?}"),
                        Err(payload) => match classify_event_unwind(payload) {
                            // `handle_event()` absorbs cancellation from handlers.
                            EventUnwind::Cancelled => {},
                            EventUnwind::Panicked(payload) => {
                                // `report_crash()` reads only the `Client` handle. Drop `self` after
                                // the panic because a handler may have partially written its state.
                                let message = panic::message(&payload);
                                crate::log_error!("Panic while handling event: {message}");
                                service_context.set_crashed();
                                report_crash(&self.client).await;
                                let _ = server_shutdown_tx.send(()).await;
                                break;
                            },
                        },
                    }
                }
            }
        }
    }

    /// Pull the next event off the channel. Only the harness session and test
    /// pumps use this; the running loop selects on the channel directly so it
    /// can also watch for shutdown.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) async fn next_event(&mut self) -> Event<H> {
        self.events_rx.recv().await.unwrap()
    }

    /// Keep event handling and its deferred refresh inside the same recovery
    /// boundary in production and the harness. Callers decide whether a panic
    /// shuts down the LSP or fails a test.
    async fn handle_event_in_boundary(
        &mut self,
        event: Event<H>,
    ) -> Result<(anyhow::Result<()>, DispatchTiming), panic::PanicPayload> {
        panic::catch_unwind_async_payload(Recovery::Always, self.handle_event(event)).await
    }

    /// The `handle_event` span groups one main-loop tick for profiling, labelled
    /// with [`Event::kind()`]. Its children are the instrumented handler and,
    /// when a refresh fires, `refresh_if_due`. The span minus that refresh is
    /// the tick time outside the refresh: dispatch plus the watchdog, boundary,
    /// and refresh-eligibility checks. `DispatchTiming` is measured separately
    /// so harness timings don't depend on which tracing layers are installed.
    #[tracing::instrument(level = "info", skip_all, fields(kind = event.kind()))]
    async fn handle_event(&mut self, event: Event<H>) -> (anyhow::Result<()>, DispatchTiming) {
        let _tick = self
            .lsp_state
            .watchdog
            .tick(self.world.db().outstanding_holds());

        // Recover handler cancellation here so `refresh_if_due()` still runs
        // inside the caller's panic boundary, as Salsa access requires. Recovering
        // in `main_loop()` would put the refresh outside its per-event boundary.
        // Other panics skip the refresh because the handler may have partially
        // written its state.
        let start = std::time::Instant::now();
        let result =
            match panic::catch_unwind_async_payload(Recovery::Always, self.dispatch_event(event))
                .await
            {
                Ok(result) => result,
                Err(payload) => match classify_event_unwind(payload) {
                    EventUnwind::Cancelled => Ok(()),
                    EventUnwind::Panicked(payload) => std::panic::resume_unwind(payload),
                },
            };
        let timing = DispatchTiming {
            start,
            end: std::time::Instant::now(),
        };

        // An event that empties the queue must trigger any deferred refresh,
        // even if its handler returns an error or is cancelled. Otherwise,
        // diagnostics could wait indefinitely for another event.
        self.refresh_if_due();

        (result, timing)
    }

    #[rustfmt::skip]
    /// Events use _exclusive_, _sharing_, or _concurrent_ handlers.
    ///
    /// - Exclusive handlers are passed an `&mut` to the world state so they can
    ///   update it.
    /// - Sharing handlers are passed a simple reference. In principle we could
    ///   run these concurrently but we run these one handler at a time for simplicity.
    /// - When concurrent handlers are needed for performance reason (one tick
    ///   of the main loop should be as fast as possible to increase throughput)
    ///   they run on the [`crate::analysis`] pool over a snapshot of the
    ///   state.
    async fn dispatch_event(&mut self, event: Event<H>) -> anyhow::Result<()> {
        let loop_tick = std::time::Instant::now();

        match event {
            Event::Lsp(msg) => match msg {
                LspMessage::Notification(notif) => {
                    crate::log_info!("{notif:#?}");
                    crate::log_info!(
                        "Entering notification handler with {n} outstanding Salsa db holds",
                        n = self.world.db().outstanding_holds()
                    );

                    match notif {
                        LspNotification::Initialized(_params) => {
                            state_handlers::handle_initialized(&self.client, &mut self.lsp_state, &mut self.world, &self.events_tx).await;
                        },
                        LspNotification::DidChangeWorkspaceFolders(params) => {
                            state_handlers::did_change_workspace_folders(params, &mut self.world, &mut self.lsp_state, &self.events_tx)?;
                        },
                        LspNotification::DidChangeConfiguration(params) => {
                            state_handlers::did_change_configuration(params, &self.client, &self.lsp_state.capabilities, &mut self.world).await?;
                        },
                        LspNotification::DidChangeWatchedFiles(params) => {
                            state_handlers::did_change_watched_files(params, &mut self.world, &mut self.lsp_state, &self.events_tx)?;
                        },
                        LspNotification::DidOpenTextDocument(params) => {
                            state_handlers::did_open(params, &mut self.world)?;
                        },
                        LspNotification::DidChangeTextDocument(params) => {
                            state_handlers::did_change(params, &mut self.handlers, &mut self.world)?;
                        },
                        LspNotification::DidSaveTextDocument(_params) => {
                            // Currently ignored
                        },
                        LspNotification::DidCloseTextDocument(params) => {
                            state_handlers::did_close(params, &mut self.world)?;
                        },
                        LspNotification::DidOpenNotebookDocument(params) => {
                            state_handlers::did_open_notebook(params, &mut self.world)?;
                        },
                        LspNotification::DidChangeNotebookDocument(params) => {
                            state_handlers::did_change_notebook(params, &mut self.handlers, &mut self.world)?;
                        },
                        LspNotification::DidCloseNotebookDocument(params) => {
                            state_handlers::did_close_notebook(params, &mut self.world)?;
                        },

                        #[cfg(feature = "testing")]
                        LspNotification::TestPanic => {
                            panic!("Test panic in a notification handler");
                        },
                    }
                },

                LspMessage::Request(request, tx) => {
                    crate::log_info!("{request:#?}");

                    let request_kind = request_kind(&request);
                    let host = &self.handlers;
                    let outcome = match request {
                        LspRequest::Initialize(params) => {
                            respond_exclusive(tx, || state_handlers::initialize(params, host, &mut self.lsp_state, &mut self.world, &self.events_tx), LspResponse::Initialize)?
                        },
                        LspRequest::WorkspaceSymbol(params) => {
                            respond(tx, || host.workspace_symbol(params, &self.request_context()), LspResponse::WorkspaceSymbol)?
                        },
                        LspRequest::DocumentSymbol(params) => {
                            respond(tx, || host.document_symbol(params, &self.request_context()), LspResponse::DocumentSymbol)?
                        },
                        LspRequest::FoldingRange(params) => {
                            respond(tx, || host.folding_range(params, &self.request_context()), LspResponse::FoldingRange)?
                        },
                        LspRequest::ExecuteCommand(_params) => {
                            let response = handlers::handle_execute_command(&self.client).await;
                            respond(tx, || response, LspResponse::ExecuteCommand)?
                        },
                        LspRequest::Completion(params) => {
                            respond(tx, || host.completion(params, &self.request_context()), LspResponse::Completion)?
                        },
                        LspRequest::CompletionResolve(params) => {
                            respond(tx, || host.completion_resolve(params, &self.request_context()), LspResponse::CompletionResolve)?
                        },
                        LspRequest::Hover(params) => {
                            respond(tx, || host.hover(params, &self.request_context()), LspResponse::Hover)?
                        },
                        LspRequest::SignatureHelp(params) => {
                            respond(tx, || host.signature_help(params, &self.request_context()), LspResponse::SignatureHelp)?
                        },
                        LspRequest::GotoDefinition(params) => {
                            respond(tx, || host.goto_definition(params, &self.request_context()), LspResponse::GotoDefinition)?
                        },
                        LspRequest::GotoImplementation(_params) => {
                            // TODO
                            respond(tx, || Ok(None), LspResponse::GotoImplementation)?
                        },
                        LspRequest::SelectionRange(params) => {
                            respond(tx, || host.selection_range(params, &self.request_context()), LspResponse::SelectionRange)?
                        },
                        LspRequest::References(params) => {
                            respond(tx, || host.references(params, &self.request_context()), LspResponse::References)?
                        },
                        LspRequest::PrepareRename(params) => {
                            respond(tx, || host.prepare_rename(params, &self.request_context()), LspResponse::PrepareRename)?
                        },
                        LspRequest::Rename(params) => {
                            respond(tx, || host.rename(params, &self.request_context()), LspResponse::Rename)?
                        },
                        LspRequest::OnTypeFormatting(params) => {
                            if let Some(path) = params.text_document_position.text_document.uri.to_document_path().log_err() {
                                state_handlers::did_change_formatting_options(&path, &params.options, &mut self.world);
                            }
                            respond(tx, || host.on_type_formatting(params, &self.request_context()), LspResponse::OnTypeFormatting)?
                        },
                        LspRequest::CodeAction(params) => {
                            respond(tx, || host.code_action(params, &self.request_context()), LspResponse::CodeAction)?
                        },
                        LspRequest::Host(request) => {
                            respond(tx, || host.request(request, &self.request_context()), LspResponse::Host)?
                        },

                        #[cfg(feature = "testing")]
                        LspRequest::TestPanic => {
                            respond(tx, || -> LspResult<()> { panic!("Test panic in a request handler") }, LspResponse::TestPanic)?
                        },
                    };

                    if outcome == RequestOutcome::Panicked &&
                        self.reported_request_panics.insert(request_kind)
                    {
                        report_request_panic(&self.client).await;
                    }
                },
            },

            Event::Host(event) => {
                let mut cx = EventContext { world: &mut self.world };
                self.handlers.event(event, &mut cx)?;
            },

            Event::OakScanCompleted(scan) => {
                crate::log_info!("Received `OakScanCompleted`");

                // This scan ran on a background task, but it sends its result
                // back here so the write happens on the main loop. Keep it that
                // way: Only the main loop should write to the oak DB (not
                // enforced by types unfortunately). Consequences of infringement:
                //
                // - It would deadlock on writes. Salsa makes a writer block
                //   until every other snapshot handle has dropped (`clones ==
                //   1`), and the main loop holds a handle that never drops. So a
                //   background writer would wait forever.
                //
                // - It would panic on reads. A salsa write cancels every
                //   in-flight read. The main loop is the sole writer and runs
                //   serially, so it never wraps its own reads in
                //   `catch_cancellation()`. A background write would cancel those
                //   reads out from under it, causing a cancellation panic.
                self.apply_scan_outcome(|scheduler, db| scheduler.apply_scan_completed(db, scan));
            },

            Event::OakScanFailed(root) => {
                crate::log_warn!(
                    "Received `OakScanFailed` for {path}",
                    path = root.path(self.world.db())
                );
                self.apply_scan_outcome(|scheduler, db| scheduler.apply_scan_failed(db, root));
            },

            Event::SourceCompleted(SourceCompleted {
                package,
                description_revision,
                response,
            }) => {
                self.log_source_completed(package, &response);

                let skipped = matches!(response, SourceResponse::Skipped);

                if let Some(directory) = self.lsp_state.source_scheduler.finish(
                    self.world.db(),
                    package,
                    description_revision,
                    response,
                ) {
                    // Ignore sources for a `package` no live root holds. A library
                    // rescan may have dropped it during the fetch. Ingesting a
                    // late result would list files under a package analysis
                    // can't see, and could take them from a live package that
                    // uses the same cached source paths.
                    if self.world.db().root_by_package(package).is_some() {
                        self.world.db_mut().set_package_sources(package, &directory);
                    }
                }

                // Schedule a skipped package immediately. `finish()` removes it
                // without advancing the Salsa revision, so tick-end scheduling cannot
                // retry it after fetching is re-enabled.
                if skipped {
                    self.schedule_sources();
                }
            },

            Event::SidecarCompleted(completed) => {
                let packages = self
                    .lsp_state
                    .sidecar_scheduler
                    .finish(self.world.db(), completed);

                let mut lazy_data_packages = 0;

                for facts in packages {
                    if facts.primitive_stubs.is_some() {
                        let name = facts.package.name(self.world.db());
                        crate::log_info!("Ingesting primitive stubs for package {name}");
                    }
                    if facts.lazy_data.is_some() {
                        lazy_data_packages += 1;
                    }

                    self.world.db_mut().set_package_facts(
                        facts.package,
                        facts.primitive_stubs.as_deref(),
                        facts.lazy_data.unwrap_or_default(),
                    );
                }

                crate::log_info!("Ingested lazy data names for {lazy_data_packages} packages");

                // If the revision was already refreshed, a completion with no
                // facts does not trigger `refresh_if_due()`. Packages that
                // changed during the batch still need another request.
                self.schedule_sidecar();
            },

            Event::EnvironmentResolved(completed) => {
                crate::log_info!("Received `EnvironmentResolved`");
                self.lsp_state.environment_scheduler.finish(
                    self.world.db_mut(),
                    completed,
                    &self.lsp_state.environment_pool,
                    &self.events_tx,
                );
            },

            Event::DiagnosticsReady(DiagnosticsReady { generation, publication }) => {
                crate::log_info!(
                    "Received `DiagnosticsReady` for {}",
                    publication.uri.as_str()
                );

                if self.lsp_state.diagnostics.accept(&publication.path, generation) {
                    self.lsp_state.diagnostics_metrics.record_accepted();
                    crate::publish_diagnostics(publication);
                } else {
                    self.lsp_state.diagnostics_metrics.record_stale();
                    let path = &publication.path;
                    let published = self.lsp_state.diagnostics.published_generation(path);
                    tracing::trace!(
                        "Dropping stale diagnostics for {path}: generation {generation} is older than published generation {published:?}"
                    );
                }

                self.lsp_state
                    .diagnostics_metrics
                    .log_snapshot(&self.lsp_state.analysis_pool);
            },

            #[cfg(feature = "testing")]
            Event::TestPanicMainLoop => unreachable!(),
        }

        // Several handlers can change `LibraryRoots`. Checking here keeps the
        // watcher registration current without each writer reporting changes.
        state_handlers::update_library_watchers(&self.client, &mut self.lsp_state, &self.world)
            .await;

        crate::log_info!("Finished handling event in {}ms", loop_tick.elapsed().as_millis());

        // TODO: Make this threshold configurable by the client
        if loop_tick.elapsed() > std::time::Duration::from_millis(50) {
            crate::log_info!("Handler took more than 50ms");
        }

        Ok(())
    }

    /// Coalesce refreshes while events are queued to avoid starting diagnostics
    /// that subsequent writes would cancel. This is not debouncing. A revision
    /// change refreshes immediately if the queue is empty, and
    /// [`MAX_REFRESH_DELAY`] limits deferral while a backlog remains.
    ///
    /// Revision tracking covers writes to diagnostics dependencies, including
    /// workspace symbols, imports, and resolved definitions, without requiring
    /// each writer to request a refresh. Config and session scopes live outside
    /// oak, so `update_config()` and [`WorldState::set_session_scopes()`] call
    /// [`WorldState::bump_revision()`] to invalidate
    /// diagnostics through the same mechanism.
    ///
    /// Tracking the last refreshed revision keeps deferred work pending across
    /// failed or interrupted events and prevents duplicate refreshes when the
    /// revision is unchanged.
    fn refresh_if_due(&mut self) {
        let revision = self.world.revision();
        if revision == self.last_refreshed_revision {
            return;
        }

        let deferred_since = *self
            .refresh_deferred_since
            .get_or_insert_with(std::time::Instant::now);
        if !self.events_rx.is_empty() && deferred_since.elapsed() < MAX_REFRESH_DELAY {
            return;
        }

        // Entered only once a refresh fires. Most events return early above,
        // and a span per skipped check would bury the real refreshes in
        // profiles.
        let _span = tracing::info_span!("refresh_if_due").entered();

        // `schedule_sidecar()` writes `Package::facts_revision()` for packages
        // with no facts to compute. Record the revision and start diagnostics
        // after those writes, so the refresh includes them rather than being
        // cancelled by them.
        self.schedule_sidecar();

        crate::log_info!("World state revision advanced");
        self.last_refreshed_revision = self.world.revision();
        self.refresh_deferred_since = None;

        let tasks = self.lsp_state.diagnostics.refresh_all(
            &self.world,
            &self.handlers,
            &self.lsp_state.analysis_pool,
            &self.events_tx,
        );
        self.lsp_state.diagnostics_metrics.record_batch(tasks);

        // Empty batches emit no `DiagnosticsReady`, so log their initial
        // snapshot here.
        self.lsp_state
            .diagnostics_metrics
            .log_snapshot(&self.lsp_state.analysis_pool);

        self.schedule_sources();
    }

    /// Apply a finished or failed scan through `apply`, then dispatch the
    /// follow-up scans it requests and resolve the environments it discovers.
    fn apply_scan_outcome(
        &mut self,
        apply: impl FnOnce(&mut ScanScheduler, &mut OakDatabase) -> Vec<ScanRequest>,
    ) {
        let followups = apply(&mut self.lsp_state.oak_scheduler, self.world.db_mut());

        // An applied workspace scan can add or remove environment directories.
        // Reconciling after library scans and failed scans writes nothing.
        self.lsp_state.environment_scheduler.reconcile(
            self.world.db_mut(),
            &self.lsp_state.environment_pool,
            &self.events_tx,
        );

        crate::log_info!(
            "Dispatching {n} followup scan requests with {n_holds} outstanding Salsa db holds",
            n = followups.len(),
            n_holds = self.world.db().outstanding_holds(),
        );

        dispatch_scan_requests(&self.lsp_state.scan_pool, &self.events_tx, followups);

        // Warm the workspace symbol index once scans settle. The oak semantic
        // index is warmed separately, as a side effect of the diagnostics
        // refresh in `refresh_if_due()`.
        if !self.lsp_state.oak_scheduler.has_pending_scans() {
            analysis::warm_workspace_index(&self.world, &self.lsp_state.analysis_pool);
        }
    }

    fn schedule_sources(&mut self) {
        self.lsp_state.source_scheduler.schedule(
            self.world.db(),
            &self.world.config.oak,
            &self.lsp_state.source_pool,
            &self.events_tx,
        );
    }

    /// Submit a sidecar batch unless one is in flight, and record coverage for
    /// packages with no facts to compute, even while a batch is in flight.
    fn schedule_sidecar(&mut self) {
        let covered = self.lsp_state.sidecar_scheduler.schedule(
            self.world.db(),
            &self.lsp_state.sidecar_pool,
            &self.events_tx,
        );

        for package in covered {
            self.world
                .db_mut()
                .set_package_facts(package, None, Vec::new());
        }
    }

    fn request_context(&self) -> RequestContext<'_> {
        RequestContext {
            world: &self.world,
            capabilities: &self.lsp_state.capabilities,
            readiness: self.lsp_state.readiness(&self.world),
        }
    }

    fn log_source_completed(&self, package: Package, response: &SourceResponse) {
        let name = package.name(self.world.db());

        match response {
            SourceResponse::Success {
                origin: SourceOrigin::Cached,
                ..
            } => {
                crate::log_info!("Sources for package {name} came from the cache")
            },
            SourceResponse::Success {
                origin: SourceOrigin::Fetched,
                ..
            } => {
                crate::log_info!("Fetched sources for package {name}")
            },
            SourceResponse::Failure => {
                crate::log_info!("Found no sources for package {name}")
            },
            SourceResponse::Skipped => {
                crate::log_info!(
                    "Skipped sources for package {name}, source fetching was turned off"
                )
            },
        }
    }
}

/// Send via `request::ShowMessageRequest` not `notification::ShowMessage` so
/// that we can ensure that the message has been received on the frontend side.
/// We are about to shut the LSP down, and sending out a fire-and-forget
/// notification often won't get sent out before shutdown occurs. The request
/// returns control to us when the user acknowledges the message. It doesn't
/// matter if that takes awhile because we shut down right after, and we've
/// already marked the service as crashed in its [`LspServiceContext`]. We do
/// bound it with a 5 second timeout just in case the user ignores the message
/// entirely, so we can still shutdown.
pub(crate) async fn report_crash(client: &Client) {
    let user_message = concat!(
        "The R language server has crashed and has been disabled. ",
        "Smart features such as completions will no longer work in this session. ",
        "Please report this crash to https://github.com/posit-dev/positron/issues ",
        "with full logs (see https://positron.posit.co/troubleshooting.html#python-and-r-logs)."
    );
    let request = client.send_request::<request::ShowMessageRequest>(ShowMessageRequestParams {
        typ: MessageType::ERROR,
        message: String::from(user_message),
        actions: None,
    });
    match tokio::time::timeout(Duration::from_secs(5), request).await {
        Ok(result) => {
            result.log_err();
        },
        Err(_) => {
            log::warn!("Timed out waiting for frontend to acknowledge LSP crash notification");
        },
    }
}

async fn report_request_panic(client: &Client) {
    client
        .show_message(
            MessageType::ERROR,
            concat!(
                "An R language server feature encountered an internal error. ",
                "The request failed, but the language server is still running. ",
                "See the R Kernel and R Language Server logs for the panic and backtrace."
            ),
        )
        .await;
}

/// Build the LSP's [`SourceHandler`] around the R executable `r`, or return
/// `None` to disable fetching. The CI opt-out applies only to source fetching,
/// not to environment resolution.
fn source_handler(r: &Path) -> Option<Arc<dyn SourceHandler>> {
    // Also reported to the LSP output channel from `handle_initialized()`.
    if source_fetching_disabled_by_ci() {
        log::info!(
            "Source fetching is disabled on CI. Set {OAK_SOURCE_FETCHING_ENABLED_ENV_VAR}=1 to enable it."
        );
        return None;
    }

    match OakSourceHandler::new(r.to_path_buf()) {
        Ok(handler) => Some(Arc::new(handler)),
        Err(err) => {
            log::error!(
                "Can't create package source handler, source fetching is disabled: {err:?}"
            );
            None
        },
    }
}

/// Unlike [`source_handler()`], this also enables the sidecar job in CI,
/// because generating primitive stubs and reading lazy data names need no network.
fn sidecar_handler(r: &Path) -> Option<Arc<dyn SidecarHandler>> {
    match OakSidecarHandler::new(r.to_path_buf()) {
        Ok(handler) => Some(Arc::new(handler)),
        Err(err) => {
            log::error!("Can't create the sidecar handler, the sidecar job is disabled: {err:?}");
            None
        },
    }
}

/// Harness access to the main loop without R or a live LSP connection.
///
/// These methods require private channels and dispatch through the real
/// `handle_event()`. [`crate::harness::LspSession`] builds on them, so
/// benchmarks built with the `testing` feature need them too.
#[cfg(any(test, feature = "testing"))]
impl<H: Handlers> GlobalState<H> {
    /// Run one `event` through `handle_event()` without pumping follow-up work.
    /// This allows callers to inspect or gate a subsystem before it settles.
    /// Panics unwind to the test runner rather than shutting down the LSP.
    ///
    /// `handle_event()` absorbs handler cancellation. Any other unwind fails the
    /// test, including a cancellation that `main_loop()` would ignore.
    pub async fn handle_event_once(&mut self, event: Event<H>) -> DispatchTiming {
        match self.handle_event_in_boundary(event).await {
            Ok((Ok(()), timing)) => timing,
            Ok((Err(err), _timing)) => panic!("Failure while handling event: {err:?}"),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub fn lsp_state(&self) -> &LspState {
        &self.lsp_state
    }

    pub fn world(&self) -> &WorldState {
        &self.world
    }

    /// Report whether no scheduler, analysis, or main-loop work is pending.
    /// This is only a predicate, so callers must still check background failures
    /// and re-check it while waiting, since the pool can go idle without
    /// sending an event.
    pub(crate) fn is_settled(&self) -> bool {
        if self.lsp_state.oak_scheduler.has_pending_scans() ||
            self.lsp_state.source_scheduler.has_pending() ||
            self.lsp_state.sidecar_scheduler.has_pending() ||
            self.lsp_state.environment_scheduler.has_pending()
        {
            return false;
        }

        // A refresh deferred by `refresh_if_due()` is pending work even though
        // nothing has been queued for it yet.
        let revision = self.world.revision();
        if revision != self.last_refreshed_revision {
            return false;
        }

        self.lsp_state.analysis_pool.is_idle() && self.events_rx.is_empty()
    }
}

/// Test-only pumps for driving the main loop to a chosen point.
#[cfg(any(test, feature = "testing"))]
impl<H: Handlers> GlobalState<H> {
    /// Run `event` through the real `handle_event`, then pump any pending
    /// events until we reach quiescence. This includes:
    /// - Pending oak scans
    /// - Pending source requests
    /// - The running sidecar job
    /// - Pending environment resolutions
    pub async fn handle_event_to_quiescence(&mut self, event: Event<H>) {
        self.handle_event_once(event).await;
        self.pump_to_quiescence().await;
    }

    /// Pump events until no scan, source request, sidecar job or environment
    /// resolution is pending.
    pub async fn pump_to_quiescence(&mut self) {
        while self.lsp_state.oak_scheduler.has_pending_scans() ||
            self.lsp_state.source_scheduler.has_pending() ||
            self.lsp_state.sidecar_scheduler.has_pending() ||
            self.lsp_state.environment_scheduler.has_pending() ||
            self.refresh_owed()
        {
            let event = self.next_event_or_panic("quiescence").await;
            self.handle_event_once(event).await;
        }
    }

    #[cfg(test)]
    /// Pump events until no oak scan is pending, ignoring pending source
    /// requests.
    pub(crate) async fn pump_scans_to_quiescence(&mut self) {
        while self.lsp_state.oak_scheduler.has_pending_scans() || self.refresh_owed() {
            let event = self.next_event_or_panic("pending scans").await;
            self.handle_event_once(event).await;
        }
    }

    /// Whether `refresh_if_due()` deferred a refresh because events were
    /// queued. Source and sidecar requests are scheduled from the refresh, so
    /// a pump that stops here would miss work the scan made due. Pumping
    /// can't stall: a deferral implies a queued event.
    fn refresh_owed(&self) -> bool {
        self.world.revision() != self.last_refreshed_revision
    }

    /// Pump events until no source request is pending, ignoring pending scans.
    pub async fn pump_sources_to_quiescence(&mut self) {
        while self.lsp_state.source_scheduler.has_pending() {
            let event = self.next_event_or_panic("pending source requests").await;
            self.handle_event_once(event).await;
        }
    }

    /// If the sidecar job is running, handle events until its response arrives.
    /// Do not wait for pending scans or source requests to finish.
    pub async fn pump_sidecar_to_quiescence(&mut self) {
        while self.lsp_state.sidecar_scheduler.has_pending() {
            let event = self.next_event().await;
            self.handle_event_once(event).await;
        }
    }

    #[cfg(test)]
    /// Pump events until one satisfies `wanted`, handling the others, and hand
    /// that one back unhandled. Lets a test slip its own events in front of a
    /// response that is already in flight.
    pub(crate) async fn take_event(&mut self, wanted: impl Fn(&Event<H>) -> bool) -> Event<H> {
        loop {
            let event = self.next_event_or_panic("a wanted event").await;
            if wanted(&event) {
                return event;
            }
            self.handle_event_once(event).await;
        }
    }

    /// [`Self::next_event()`] with a deadline. The test pumps wait for events
    /// that work in flight will send, so work that was never dispatched (a
    /// fixture package the scanner didn't list, a lost wakeup) would otherwise
    /// block the test forever: the local nextest profile reports slow tests
    /// but never kills them.
    async fn next_event_or_panic(&mut self, waiting_for: &str) -> Event<H> {
        const TIMEOUT: Duration = Duration::from_secs(30);
        match tokio::time::timeout(TIMEOUT, self.next_event()).await {
            Ok(event) => event,
            Err(_) => panic!("No event within {TIMEOUT:?} while waiting for {waiting_for}"),
        }
    }
}

/// When `dispatch_event()` started and returned for one event. Captured in
/// `handle_event()` so the `refresh_if_due()` that follows dispatch is
/// excluded from handler timings.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(
    not(any(test, feature = "testing")),
    expect(dead_code, reason = "only the harness reads the timing")
)]
pub struct DispatchTiming {
    pub(crate) start: std::time::Instant,
    pub(crate) end: std::time::Instant,
}

enum EventUnwind {
    Cancelled,
    Panicked(panic::PanicPayload),
}

fn classify_event_unwind(payload: panic::PanicPayload) -> EventUnwind {
    if payload.is::<salsa::Cancelled>() {
        EventUnwind::Cancelled
    } else {
        EventUnwind::Panicked(payload)
    }
}

/// Run each [`ScanRequest`] on `pool`. Each job runs the pure-I/O
/// [`ScanRequest::run`] and ships the [`ScanCompleted`] back to the main loop as
/// [`Event::OakScanCompleted`], where the scheduler then applies it.
///
/// A scan that panics or never runs reports [`Event::OakScanFailed`]. Without
/// that notification, the root would remain `Scanning`, preventing buffered
/// watcher events under it from applying.
pub(super) fn dispatch_scan_requests<H: Handlers>(
    pool: &IoPool,
    events_tx: &TokioUnboundedSender<Event<H>>,
    requests: Vec<ScanRequest>,
) {
    for req in requests {
        let on_abort = Event::OakScanFailed(req.root);
        pool.submit_reporting(events_tx, on_abort, move || {
            Event::OakScanCompleted(req.run())
        });
    }
}

/// Deduplication key for request panic reports. For [`LspRequest::Host`],
/// include the host request's discriminant to distinguish its enum variants.
type RequestKind<H> = (
    Discriminant<LspRequest<H>>,
    Option<Discriminant<<H as Handlers>::Request>>,
);

fn request_kind<H: Handlers>(request: &LspRequest<H>) -> RequestKind<H> {
    let host_kind = match request {
        LspRequest::Host(request) => Some(discriminant(request)),
        _ => None,
    };
    (discriminant(request), host_kind)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestOutcome {
    Handled,
    Panicked,
}

/// Respond to a request from the LSP
///
/// We receive requests from the LSP client with a response channel. Once we
/// have a response, we send it to tower-lsp which will forward it to the
/// client.
///
/// The response channel will be closed if the request has been cancelled on
/// the tower-lsp side. In that case the future of the async request method
/// has been dropped, along with the receiving side of this channel. It's
/// unclear whether we want to support this sort of client-side cancellation
/// better. We should probably focus on cancellation of expensive tasks
/// running on side threads when the world state has changed.
///
/// # Arguments
///
/// * - `response_tx`: A response channel for the tower-lsp request handler.
/// * - `response`: A closure producing a response wrapped in a `LspResult`. Errors are logged.
/// * - `into_lsp_response`: A constructor for the relevant `LspResponse` variant.
fn respond<H: Handlers, T>(
    response_tx: TokioUnboundedSender<RequestResponse<H>>,
    response: impl FnOnce() -> LspResult<T>,
    into_lsp_response: impl FnOnce(T) -> LspResponse<H>,
) -> anyhow::Result<RequestOutcome> {
    let (response, outcome) = match panic::catch_unwind(Recovery::Always, || {
        catch_cancellation(response)
    }) {
        Ok(Some(Ok(value))) => (
            RequestResponse::Result(Ok(into_lsp_response(value))),
            RequestOutcome::Handled,
        ),
        Ok(Some(Err(err))) => (RequestResponse::Result(Err(err)), RequestOutcome::Handled),
        Ok(None) => {
            // A salsa write cancelled an oak query while the handler ran.
            // Report `ContentModified` so the client knows the content moved
            // under us and re-requests.
            (
                RequestResponse::Result(Err(LspError::JsonRpc(jsonrpc::Error::content_modified()))),
                RequestOutcome::Handled,
            )
        },
        Err(message) => {
            // The panic hook emits the backtrace to the kernel logs. Mention
            // the panic in the LSP log too for cross-reference.
            crate::log_error!(
                "Panic while handling request: {message}. \
                 See the R kernel log for the full panic backtrace."
            );
            (
                RequestResponse::Result(Err(LspError::Anyhow(anyhow!(
                    "Panic while handling request: {message}"
                )))),
                RequestOutcome::Panicked,
            )
        },
    };

    send_response(response_tx, response)?;
    Ok(outcome)
}

/// Run a handler that holds an exclusive world-state borrow.
///
/// We don't recover from panics in these handlers because they could leave
/// leave the world state half-built. Let them reach `main_loop()`'s recovery
/// boundary, which terminates the session.
fn respond_exclusive<H: Handlers, T>(
    response_tx: TokioUnboundedSender<RequestResponse<H>>,
    response: impl FnOnce() -> LspResult<T>,
    into_lsp_response: impl FnOnce(T) -> LspResponse<H>,
) -> anyhow::Result<RequestOutcome> {
    let response = match catch_cancellation(response) {
        Some(Ok(value)) => RequestResponse::Result(Ok(into_lsp_response(value))),
        Some(Err(err)) => RequestResponse::Result(Err(err)),
        None => RequestResponse::Result(Err(LspError::JsonRpc(jsonrpc::Error::content_modified()))),
    };

    send_response(response_tx, response)?;
    Ok(RequestOutcome::Handled)
}

fn send_response<H: Handlers>(
    response_tx: TokioUnboundedSender<RequestResponse<H>>,
    response: RequestResponse<H>,
) -> anyhow::Result<()> {
    let out = match response {
        RequestResponse::Result(Ok(_)) => Ok(()),
        RequestResponse::Result(Err(ref error)) => {
            // The error has already been sent to the client on `response_tx`
            // as a jsonrpc error, so the user sees the popup. Log here at
            // info level (with `{:?}` for the full debug format including a
            // backtrace) so server logs keep diagnostic context.
            crate::log_info!("Error while handling request:\n{error:?}");
            Ok(())
        },
        RequestResponse::Disabled => Err(anyhow!("Received impossible `Disabled` response state")),
    };

    // Ignore errors from a closed channel. This indicates the request has
    // been cancelled on the tower-lsp side.
    let _ = response_tx.send(response);

    out
}

impl AuxiliaryState {
    /// Create the event handler without binding it to a receiver. Tests can
    /// then drive production publication and deduplication one event at a time.
    fn new(client: Client) -> Self {
        Self {
            client,
            published_diagnostics: HashMap::new(),
        }
    }

    async fn start(mut self, mut auxiliary_event_rx: TokioUnboundedReceiver<AuxiliaryEvent>) {
        loop {
            match panic::catch_unwind_async(
                Recovery::Always,
                self.handle_next_event(&mut auxiliary_event_rx),
            )
            .await
            {
                Ok(ControlFlow::Continue(())) => {},
                Ok(ControlFlow::Break(())) => break,
                // Use `log::error!()` instead of `crate::log_error!()`, which queues another event
                // on this loop. A panic in `log()` would otherwise recurse indefinitely.
                Err(msg) => log::error!("Panic in the auxiliary loop: {msg}"),
            }
        }
    }

    async fn handle_next_event(
        &mut self,
        auxiliary_event_rx: &mut TokioUnboundedReceiver<AuxiliaryEvent>,
    ) -> ControlFlow<()> {
        match next_auxiliary_event(auxiliary_event_rx).await {
            AuxiliaryEvent::Log(level, message) => self.log(level, message).await,
            AuxiliaryEvent::PublishDiagnostics(publication) => {
                self.publish_diagnostics(publication).await
            },
            AuxiliaryEvent::ReportBackgroundPanic => self.report_background_panic().await,
            AuxiliaryEvent::Shutdown => return ControlFlow::Break(()),
            #[cfg(feature = "testing")]
            AuxiliaryEvent::TestPanic => panic!("Test panic in the auxiliary loop"),
        }

        ControlFlow::Continue(())
    }

    async fn report_background_panic(&self) {
        self.client
            .show_message(
                MessageType::ERROR,
                String::from(
                    "An R language server background task encountered an internal error. \
                     Some smart features may be temporarily unavailable. \
                     See https://positron.posit.co/troubleshooting.html#python-and-r-logs \
                     for full logs and report the problem at \
                     https://github.com/posit-dev/positron/issues.",
                ),
            )
            .await;
    }

    /// Publish diagnostics, skipping the client round-trip when the set is
    /// identical to what we last sent for that file. Only non-empty sets are
    /// remembered, and an absent entry counts as empty. So an empty result is
    /// published only when it clears diagnostics the client is currently
    /// showing, and the map stays bounded by the files on screen with
    /// diagnostics.
    async fn publish_diagnostics(&mut self, publication: DiagnosticsPublication) {
        let DiagnosticsPublication {
            path,
            uri,
            diagnostics,
            version,
        } = publication;

        let unchanged = match self.published_diagnostics.get(&path) {
            Some(old) => diagnostics == *old,
            None => diagnostics.is_empty(),
        };
        if unchanged {
            return;
        }

        self.client
            .publish_diagnostics(uri, diagnostics.clone(), version)
            .await;

        if diagnostics.is_empty() {
            self.published_diagnostics.remove(&path);
        } else {
            self.published_diagnostics.insert(path, diagnostics);
        }
    }

    async fn log(&self, level: MessageType, message: String) {
        self.client.log_message(level, message).await
    }
}

/// Treat a closed receiver as shutdown. Replacing the process-global sender
/// drops the previous sender and closes an older session's receiver.
async fn next_auxiliary_event(rx: &mut TokioUnboundedReceiver<AuxiliaryEvent>) -> AuxiliaryEvent {
    match rx.recv().await {
        Some(event) => event,
        None => AuxiliaryEvent::Shutdown,
    }
}

/// Install the process-global sender used for logs and diagnostics. Replacing
/// it closes the previous receiver, which shuts down the older auxiliary loop.
pub(crate) fn register_auxiliary_tx(tx: TokioUnboundedSender<AuxiliaryEvent>) {
    let mut global = AUXILIARY_EVENT_TX.write().unwrap();
    *global = Some(tx);
}

fn with_auxiliary_tx<F, T>(f: F) -> T
where
    F: FnOnce(&TokioUnboundedSender<AuxiliaryEvent>) -> T,
{
    let auxiliary_event_tx = AUXILIARY_EVENT_TX
        .read()
        .expect("Can lock auxiliary event sender.");

    // If we get here that means the LSP was initialised at least once. The
    // channel might be closed if the LSP was dropped, but it should exist.
    let auxiliary_event_tx = auxiliary_event_tx
        .as_ref()
        .expect("LSP should have been initialized at least once by now.");

    f(auxiliary_event_tx)
}

fn send_auxiliary(event: AuxiliaryEvent) {
    with_auxiliary_tx(|auxiliary_event_tx| {
        if let Err(err) = auxiliary_event_tx.send(event) {
            // The error includes the event
            log::warn!("LSP is shut down, can't send event:\n{err:?}");
        }
    })
}

fn report_background_panic() {
    let Ok(auxiliary_event_tx) = AUXILIARY_EVENT_TX.read() else {
        log::warn!("Can't lock auxiliary event sender to report a background panic");
        return;
    };
    let Some(auxiliary_event_tx) = auxiliary_event_tx.as_ref() else {
        log::warn!("Can't report a background panic before the LSP is initialized");
        return;
    };

    if let Err(err) = auxiliary_event_tx.send(AuxiliaryEvent::ReportBackgroundPanic) {
        log::warn!("LSP is shut down, can't report a background panic:\n{err:?}");
    }
}

/// Initialise the auxiliary channel for unit tests that exercise LSP
/// handlers calling `publish_diagnostics` / `log` / similar.
///
/// Production wires the sender during `AuxiliaryState::start`. Tests don't
/// run that path, so `with_auxiliary_tx` would panic. This sets a sender
/// into the static and returns the receiver so tests can assert on events.
#[cfg(any(test, feature = "testing"))]
pub fn init_aux_for_test() -> TokioUnboundedReceiver<AuxiliaryEvent> {
    let (tx, rx) = tokio_unbounded_channel::<AuxiliaryEvent>();
    register_auxiliary_tx(tx);
    rx
}

/// Send a message to the LSP client. This is non-blocking and treated on a
/// latency-sensitive task.
pub fn log(level: lsp_types::MessageType, message: String) {
    // Check that channel is still alive in case the LSP was closed.
    // If closed, fallthrough.
    if with_auxiliary_tx(|auxiliary_event_tx| {
        auxiliary_event_tx.send(AuxiliaryEvent::Log(level, message.clone()))
    })
    .is_ok()
    {
        return;
    }

    // Log to the kernel as fallback
    log::warn!("LSP channel is closed, redirecting messages to Jupyter kernel");

    match level {
        MessageType::ERROR => log::error!("{message}"),
        MessageType::WARNING => log::warn!("{message}"),
        _ => log::info!("{message}"),
    };
}

#[cfg(feature = "testing")]
pub(crate) fn panic_auxiliary_loop() {
    send_auxiliary(AuxiliaryEvent::TestPanic);
}

pub(crate) fn publish_diagnostics(publication: DiagnosticsPublication) {
    send_auxiliary(AuxiliaryEvent::PublishDiagnostics(publication));
}

#[derive(Debug)]
pub struct DiagnosticsPublication {
    /// Identity for the dedup cache. Two spellings of the same document
    /// have the same identity.
    pub(crate) path: FilePath,
    /// Wire information.
    pub(crate) uri: Uri,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) version: Option<i32>,
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::time::Instant;

    use aether_path::FilePath;
    use oak_scan::DbScan;
    use serde_json::Value;
    use tower_lsp_server::jsonrpc;
    use tower_lsp_server::ls_types::ServerInfo;
    use url::Url;

    use super::classify_event_unwind;
    use super::init_aux_for_test;
    use super::register_auxiliary_tx;
    use super::request_kind;
    use super::respond;
    use super::send_auxiliary;
    use super::tokio_unbounded_channel;
    use super::AuxiliaryEvent;
    use super::AuxiliaryState;
    use super::Diagnostic;
    use super::DiagnosticsPublication;
    use super::Event;
    use super::EventUnwind;
    use super::GlobalState;
    use super::LspServiceContext;
    use super::LspState;
    use super::MAX_REFRESH_DELAY;
    use crate::backend::LspError;
    use crate::backend::LspRequest;
    use crate::backend::LspResponse;
    use crate::backend::LspResult;
    use crate::backend::RequestResponse;
    use crate::handlers::EventContext;
    use crate::handlers::Handlers;
    use crate::handlers::RequestContext;
    use crate::harness::editor::client::test_client;
    use crate::harness::editor::client::TestClient;
    use crate::harness::editor::notifications::did_change;
    use crate::harness::editor::notifications::did_open;
    use crate::harness::TestHandlers;
    use crate::state::WorldState;
    use crate::traits::url::UrlExt;

    #[tokio::test]
    async fn test_background_panic_is_reported_once_per_session() {
        let client = TestClient::new(&[]).await;
        let (auxiliary_event_tx, auxiliary_event_rx) = tokio_unbounded_channel::<AuxiliaryEvent>();
        register_auxiliary_tx(auxiliary_event_tx);
        let auxiliary = AuxiliaryState::new(client.client());
        // Keep an unexpected loop panic visible through `auxiliary_loop.await`.
        // `panic::spawn()` would log it and return a successful `JoinHandle`.
        #[allow(clippy::disallowed_methods)]
        let auxiliary_loop = tokio::spawn(auxiliary.start(auxiliary_event_rx));
        let first_session = LspServiceContext::new();
        let second_session = LspServiceContext::new();

        first_session.report_background_panic();
        first_session.report_background_panic();
        second_session.report_background_panic();
        send_auxiliary(AuxiliaryEvent::Shutdown);
        auxiliary_loop.await.unwrap();
        client.flush().await;

        let notifications = client.notifications();
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].0, "window/showMessage");
        assert_eq!(notifications[1].0, "window/showMessage");
        let message = notifications[0].1["message"].as_str().unwrap();
        assert!(message.contains("background task encountered an internal error"));
    }

    #[tokio::test]
    async fn test_registering_a_sender_retires_the_previous_receiver() {
        let (first_tx, mut first_rx) = tokio_unbounded_channel::<AuxiliaryEvent>();
        register_auxiliary_tx(first_tx);

        let (second_tx, mut second_rx) = tokio_unbounded_channel::<AuxiliaryEvent>();
        register_auxiliary_tx(second_tx);

        assert!(first_rx.recv().await.is_none());

        send_auxiliary(AuxiliaryEvent::Shutdown);
        assert!(matches!(
            second_rx.recv().await,
            Some(AuxiliaryEvent::Shutdown)
        ));
    }

    /// Unchanged diagnostics are suppressed, so notification counts cannot
    /// signal that refresh work completed. Empty results publish only to clear
    /// diagnostics the client is showing.
    #[tokio::test]
    async fn test_only_changed_diagnostics_reach_the_client() {
        let client = TestClient::new(&[]).await;
        let mut auxiliary = AuxiliaryState::new(client.client());

        for diagnostics in [
            vec![diagnostic("first")],
            vec![diagnostic("first")],
            vec![diagnostic("second")],
            vec![],
            vec![],
        ] {
            auxiliary
                .publish_diagnostics(publication(diagnostics))
                .await;
        }
        client.flush().await;

        let notifications = client.notifications();
        let methods: Vec<&str> = notifications
            .iter()
            .map(|(method, _params)| method.as_str())
            .collect();
        assert_eq!(methods, vec![
            "textDocument/publishDiagnostics",
            "textDocument/publishDiagnostics",
            "textDocument/publishDiagnostics",
        ]);

        let published: Vec<Vec<&str>> = notifications
            .iter()
            .map(|(_method, params)| messages(params))
            .collect();
        assert_eq!(published, vec![vec!["first"], vec!["second"], vec![]]);
    }

    fn messages(params: &Value) -> Vec<&str> {
        params["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|diagnostic| diagnostic["message"].as_str().unwrap())
            .collect()
    }

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            message: String::from(message),
            ..Default::default()
        }
    }

    fn publication(diagnostics: Vec<Diagnostic>) -> DiagnosticsPublication {
        let url = Url::parse("file:///test.R").unwrap();
        DiagnosticsPublication {
            path: FilePath::from_url(&url),
            uri: url.to_uri().unwrap(),
            diagnostics,
            version: None,
        }
    }

    #[tokio::test]
    async fn test_failing_event_still_runs_a_deferred_refresh() {
        let _aux = init_aux_for_test();
        let mut state = global_state();
        let dir = tempfile::tempdir().unwrap();
        let unopened = dir.path().join("unopened.R");

        state
            .events_tx
            .send(did_change(&unopened, "y <- 2\n", 1))
            .unwrap();
        state
            .handle_event(did_open(&dir.path().join("script.R"), "x <- 1\n"))
            .await
            .0
            .unwrap();
        assert_eq!(state.lsp_state.diagnostics_metrics.batches, 0);

        let failing = state.next_event().await;
        assert!(state.handle_event(failing).await.0.is_err());
        assert_eq!(state.lsp_state.diagnostics_metrics.batches, 1);
    }

    #[tokio::test]
    async fn test_owed_refresh_runs_after_the_delay_despite_queued_events() {
        let _aux = init_aux_for_test();
        let mut state = global_state();
        let dir = tempfile::tempdir().unwrap();

        state
            .events_tx
            .send(did_change(&dir.path().join("queued.R"), "y <- 2\n", 1))
            .unwrap();
        state
            .handle_event(did_open(&dir.path().join("script.R"), "x <- 1\n"))
            .await
            .0
            .unwrap();
        assert_eq!(state.lsp_state.diagnostics_metrics.batches, 0);

        state.refresh_deferred_since = Some(Instant::now() - MAX_REFRESH_DELAY);
        state.refresh_if_due();
        assert_eq!(state.lsp_state.diagnostics_metrics.batches, 1);
        assert!(!state.events_rx.is_empty());

        state.refresh_if_due();
        assert_eq!(state.lsp_state.diagnostics_metrics.batches, 1);
    }

    /// Cancellation of the last queued event must still trigger the refresh
    /// deferred while that event was queued. No further event may arrive to
    /// trigger it.
    #[tokio::test]
    async fn test_cancelled_event_still_runs_a_deferred_refresh() {
        let _aux = init_aux_for_test();
        let mut state = global_state();
        let dir = tempfile::tempdir().unwrap();

        state.events_tx.send(Event::Host(Cancel)).unwrap();
        state
            .handle_event(did_open(&dir.path().join("script.R"), "x <- 1\n"))
            .await
            .0
            .unwrap();
        assert_eq!(state.lsp_state.diagnostics_metrics.batches, 0);

        let cancelled = state.next_event().await;
        state.handle_event(cancelled).await.0.unwrap();
        assert_eq!(state.lsp_state.diagnostics_metrics.batches, 1);
    }

    fn global_state() -> GlobalState<CancellingHandlers> {
        GlobalState::from_parts(
            CancellingHandlers,
            test_client(),
            WorldState::default(),
            LspState::new(),
        )
    }

    /// A host whose only event unwinds with a Salsa cancellation, as a handler
    /// does when a database write cancels its query.
    #[derive(Debug)]
    struct CancellingHandlers;

    #[derive(Debug)]
    struct Cancel;

    impl Handlers for CancellingHandlers {
        type Request = Infallible;
        type Response = Infallible;
        type Event = Cancel;

        fn server_info(&self) -> ServerInfo {
            ServerInfo {
                name: String::from("Cancelling test host"),
                version: None,
            }
        }

        fn request(&self, request: Infallible, _cx: &RequestContext<'_>) -> LspResult<Infallible> {
            match request {}
        }

        fn event(&mut self, _event: Cancel, _cx: &mut EventContext<'_>) -> anyhow::Result<()> {
            std::panic::resume_unwind(Box::new(salsa::Cancelled::PendingWrite))
        }
    }

    /// A host with two request kinds, which panic reports must tell apart.
    #[derive(Debug)]
    struct TwoRequestHandlers;

    #[derive(Debug)]
    enum TwoRequests {
        First,
        Second,
    }

    impl Handlers for TwoRequestHandlers {
        type Request = TwoRequests;
        type Response = ();
        type Event = Infallible;

        fn server_info(&self) -> ServerInfo {
            ServerInfo {
                name: String::from("Two-request test host"),
                version: None,
            }
        }

        fn request(&self, _request: TwoRequests, _cx: &RequestContext<'_>) -> LspResult<()> {
            Ok(())
        }

        fn event(&mut self, event: Infallible, _cx: &mut EventContext<'_>) -> anyhow::Result<()> {
            match event {}
        }
    }

    #[test]
    fn test_request_kind_tells_host_requests_apart() {
        let first = request_kind::<TwoRequestHandlers>(&LspRequest::Host(TwoRequests::First));
        let second = request_kind::<TwoRequestHandlers>(&LspRequest::Host(TwoRequests::Second));

        assert_eq!(
            first,
            request_kind::<TwoRequestHandlers>(&LspRequest::Host(TwoRequests::First))
        );
        assert_ne!(first, second);
    }

    #[test]
    fn test_salsa_cancellation_is_not_classified_as_event_panic() {
        let payload = Box::new(salsa::Cancelled::PendingWrite);
        let outcome = classify_event_unwind(payload);

        assert!(matches!(outcome, EventUnwind::Cancelled));
    }

    #[test]
    fn test_genuine_panic_is_classified_as_event_panic() {
        let payload = Box::new(String::from("oh no"));
        let outcome = classify_event_unwind(payload);

        let EventUnwind::Panicked(payload) = outcome else {
            panic!("Expected a panic");
        };
        assert_eq!(crate::panic::message(&payload), "oh no");
    }

    /// A `salsa::Cancelled` re-raised out of a request handler (by `r_task`,
    /// after catching it on the R thread) must not crash the LSP. `respond`
    /// recognises the payload and answers `ContentModified` so the client
    /// re-requests, rather than taking the panic-is-a-crash path.
    #[test]
    fn test_cancelled_request_reports_content_modified() {
        let mut state = WorldState::default();
        let uri = Url::parse("file:///test.R").unwrap();
        let file = state
            .db_mut()
            .upsert_editor(FilePath::from_url(&uri), "foo".to_string());
        state.insert_open_file(uri.to_uri().unwrap(), FilePath::from_url(&uri), file, None);

        let file = state.open_file(&FilePath::from_url(&uri)).unwrap().clone();
        let snapshot = state.snapshot();
        snapshot.cancellation_token().cancel();

        let (response_tx, mut response_rx) =
            tokio_unbounded_channel::<RequestResponse<TestHandlers>>();
        respond(
            response_tx,
            || {
                let _ = file.tree_sitter(snapshot.db());
                Ok(LspResponse::Hover(None))
            },
            |response| response,
        )
        .unwrap();

        let response = response_rx.try_recv().unwrap();
        let RequestResponse::Result(Err(LspError::JsonRpc(error))) = response else {
            panic!("Expected a jsonrpc error response");
        };
        assert_eq!(error.code, jsonrpc::ErrorCode::ContentModified);
    }

    /// The central diagnostics refresh keys off the oak revision advancing
    /// across a loop tick, so an oak write must bump the revision. This pins
    /// that assumption: if a salsa upgrade changed it, the refresh would
    /// silently stop firing.
    #[test]
    fn test_oak_write_advances_revision() {
        let mut state = WorldState::default();
        let before = state.revision();
        state.db_mut().upsert_editor(
            FilePath::from_url(&Url::parse("file:///a.R").unwrap()),
            "x <- 1".to_string(),
        );
        let after = state.revision();
        assert_ne!(before, after);
    }

    /// Session scopes and config are not oak inputs. Their changes rely on
    /// `bump_revision()` advancing the revision to trigger the central
    /// diagnostics refresh.
    #[test]
    fn test_bump_revision_advances_revision() {
        let mut state = WorldState::default();
        let before = state.revision();
        state.bump_revision();
        let after = state.revision();
        assert_ne!(before, after);
    }
}
