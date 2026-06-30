//
// main_loop.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::collections::HashSet;
use std::future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::RwLock;

use aether_path::FilePath;
use anyhow::anyhow;
use futures::StreamExt;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use oak_scan::ScanCompleted;
use oak_scan::ScanRequest;
use oak_scan::ScanScheduler;
use stdext::result::ResultExt;
use stdext::spawn;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::sync::mpsc::unbounded_channel as tokio_unbounded_channel;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower_lsp::jsonrpc;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::MessageType;
use tower_lsp::Client;
use url::Url;

use super::backend::RequestResponse;
use crate::console::ConsoleNotification;
use crate::lsp;
use crate::lsp::backend::LspError;
use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::backend::LspRequest;
use crate::lsp::backend::LspResponse;
use crate::lsp::backend::LspResult;
use crate::lsp::capabilities::Capabilities;
use crate::lsp::diagnostics::generate_diagnostics;
use crate::lsp::handlers;
use crate::lsp::indexer;
use crate::lsp::open_file::OpenFile;
use crate::lsp::sources::OakSourceHandler;
use crate::lsp::sources::SourceCompleted;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceScheduler;
use crate::lsp::state::WorldState;
use crate::lsp::state_handlers;
use crate::lsp::state_handlers::ConsoleInputs;
use crate::url::ExtUrl;

pub(crate) type TokioUnboundedSender<T> = tokio::sync::mpsc::UnboundedSender<T>;
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

pub static LSP_HAS_CRASHED: AtomicBool = AtomicBool::new(false);

// This is the syntax for trait aliases until an official one is stabilised.
// This alias is for the future of a `JoinHandle<anyhow::Result<T>>`
trait AnyhowJoinHandleFut<T>:
    future::Future<Output = std::result::Result<anyhow::Result<T>, tokio::task::JoinError>>
{
}
impl<T, F> AnyhowJoinHandleFut<T> for F where
    F: future::Future<Output = std::result::Result<anyhow::Result<T>, tokio::task::JoinError>>
{
}

// Alias for a list of join handle futures
type TaskList<T> = futures::stream::FuturesUnordered<Pin<Box<dyn AnyhowJoinHandleFut<T> + Send>>>;

#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
pub(crate) enum Event {
    Lsp(LspMessage),
    Kernel(KernelNotification),
    OakScanCompleted(ScanCompleted),
    SourceCompleted(SourceCompleted),
}

#[derive(Debug)]
#[expect(clippy::enum_variant_names)]
pub(crate) enum KernelNotification {
    DidChangeConsoleInputs(ConsoleInputs),
    DidOpenVirtualDocument(DidOpenVirtualDocumentParams),
    DidCloseVirtualDocument(DidCloseVirtualDocumentParams),
}

/// A thin wrapper struct with a custom `Debug` method more appropriate for trace logs
pub(crate) struct TraceKernelNotification<'a> {
    inner: &'a KernelNotification,
}

#[derive(Debug)]
pub(crate) struct DidOpenVirtualDocumentParams {
    pub(crate) uri: String,
    pub(crate) contents: String,
}

#[derive(Debug)]
pub(crate) struct DidCloseVirtualDocumentParams {
    pub(crate) uri: String,
}

#[derive(Debug)]
pub(crate) enum AuxiliaryEvent {
    Log(lsp_types::MessageType, String),
    PublishDiagnostics(Url, Vec<Diagnostic>, Option<i32>),
    SpawnedTask(JoinHandle<anyhow::Result<Option<AuxiliaryEvent>>>),
    Shutdown,
}

/// Global state for the main loop
///
/// This is a singleton that fully owns the source of truth for `WorldState`
/// which contains the inputs of all LSP methods. The `main_loop()` method is
/// the heart of the LSP. The tower-lsp backend and the Jupyter kernel
/// communicate with the main loop through the `Event` channel that is passed on
/// construction.
pub(crate) struct GlobalState {
    /// The global world state containing all inputs for LSP analysis lives
    /// here. The dispatcher provides refs, exclusive refs, or snapshots
    /// (clones) to handlers.
    world: WorldState,

    /// The non-cloneable, per-session LSP state. Only used in exclusive ref
    /// handlers.
    lsp_state: LspState,

    /// LSP client shared with tower-lsp and the log loop
    client: Client,

    /// Event channels for the main loop. The tower-lsp methods forward
    /// notifications and requests here via `Event::Lsp`. We also receive
    /// messages from the kernel via `Event::Kernel`, and from ourselves via
    /// `Event::Task`.
    events_tx: TokioUnboundedSender<Event>,
    events_rx: TokioUnboundedReceiver<Event>,
}

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

/// Non-cloneable, per-session state mutated only by exclusive handlers.
/// Sits alongside [`WorldState`] (which is cloneable for snapshot
/// handlers); state that can't be cloned lives here instead.
pub(crate) struct LspState {
    /// Capabilities negotiated with the client
    pub(crate) capabilities: Capabilities,

    /// Channel for sending notifications to Console (e.g., document changes for DAP)
    pub(crate) console_notification_tx: TokioUnboundedSender<ConsoleNotification>,

    /// Coordinator for asynchronous workspace scans. Mutated only from
    /// main-loop handlers. Must be out of [`WorldState`] because the scheduler
    /// is not clonable.
    pub(crate) oak_scheduler: ScanScheduler,

    /// Scheduler of [crate::lsp::sources::SourceRequest]s. Scheduling and source
    /// consumption all happen from the main loop.
    pub(crate) source_scheduler: SourceScheduler,
}

impl LspState {
    pub(crate) fn new(
        console_notification_tx: TokioUnboundedSender<ConsoleNotification>,
        source_scheduler: SourceScheduler,
    ) -> Self {
        Self {
            capabilities: Capabilities::default(),
            console_notification_tx,
            oak_scheduler: ScanScheduler::new(),
            source_scheduler,
        }
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
/// - Joining of spawned blocking tasks to relay any errors or panics to the LSP log.
struct AuxiliaryState {
    client: Client,
    auxiliary_event_rx: TokioUnboundedReceiver<AuxiliaryEvent>,
    tasks: TaskList<Option<AuxiliaryEvent>>,
    /// Last non-empty diagnostics published per file. A refresh re-runs every
    /// open file, but most runs produce the same result, so we skip the publish
    /// when it matches what the client already has.
    published_diagnostics: HashMap<Url, Vec<Diagnostic>>,
}

impl GlobalState {
    /// Create a new global state
    ///
    /// # Arguments
    ///
    /// * `client`: The tower-lsp client shared with the tower-lsp backend
    ///   and auxiliary loop.
    pub(crate) fn new(
        client: Client,
        r_home: PathBuf,
        console_notification_tx: TokioUnboundedSender<ConsoleNotification>,
    ) -> Self {
        // FIXME: We shouldn't call R code in the kernel to figure this out
        let library_paths = crate::r_task(|| -> anyhow::Result<Vec<String>> {
            Ok(harp::RFunction::new("base", ".libPaths")
                .call()?
                .try_into()?)
        });

        let library_paths = match library_paths {
            Ok(library_paths) => library_paths,
            Err(err) => {
                log::error!("Can't evaluate `libPaths()`: {err:?}");
                Vec::new()
            },
        };

        let library_paths: Vec<PathBuf> = library_paths.into_iter().map(PathBuf::from).collect();

        let mut db = OakDatabase::new();
        db.set_library_paths(&library_paths);

        Self::from_parts(
            client,
            WorldState::new(db),
            LspState::new(
                console_notification_tx,
                SourceScheduler::new(source_handler(&r_home)),
            ),
        )
    }

    /// Assemble the state around an already-built [`WorldState`] and
    /// [`LspState`]. Splitting this out from [`GlobalState::new`] lets tests
    /// construct a state without the R `.libPaths()` lookup that `new` does, and
    /// with a db / provider configured up front.
    pub(crate) fn from_parts(client: Client, world: WorldState, lsp_state: LspState) -> Self {
        // Transmission channel for the main loop events. Shared with the
        // tower-lsp backend and the Jupyter kernel.
        let (events_tx, events_rx) = tokio_unbounded_channel::<Event>();

        Self {
            world,
            lsp_state,
            client,
            events_tx,
            events_rx,
        }
    }

    /// Get `Event` transmission channel
    pub(crate) fn events_tx(&self) -> TokioUnboundedSender<Event> {
        self.events_tx.clone()
    }

    /// Start the main and auxiliary loops.
    ///
    /// The returned [`LoopHandles`] owns everything the loops need. Drop it to
    /// shut the loops down and release the owned state.
    pub(crate) fn start(self) -> LoopHandles {
        let mut aux = tokio::task::JoinSet::<()>::new();

        // The auxiliary loop is fully async and never blocks. Must be started
        // first to initialise the global transmission channel.
        let aux_state = AuxiliaryState::new(self.client.clone());
        aux.spawn(async move { aux_state.start().await });

        // Since the main loop owns the Salsa DB and writes to it, we run on its
        // own thread instead of a Tokio worker. Salsa writes are potentially
        // blocking until the writer gains exclusive access. If background tasks
        // holding clones of the DB are stuck on the same thread as the main
        // loop, the LSP deadlocks. This can be avoided by wrapping writes in
        // `block_in_place()` but the safer structure is to have it run on an OS
        // thread that we're in control of.
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = Handle::current();
        let main_loop = spawn!("oak-main-loop", move || {
            handle.block_on(self.main_loop(shutdown_rx));
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
    async fn main_loop(mut self, mut shutdown_rx: oneshot::Receiver<()>) {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    lsp::log_info!("Main loop stopping: handle dropped");
                    break;
                },
                event = self.events_rx.recv() => {
                    let Some(event) = event else {
                        lsp::log_info!("Main loop stopping: event channel closed");
                        break;
                    };
                    if let Err(err) = self.handle_event(event).await {
                        lsp::log_error!("Failure while handling event:\n{err:?}")
                    }
                }
            }
        }
    }

    /// Pull the next event off the channel. Only the test pump uses this; the
    /// running loop selects on the channel directly so it can also watch for
    /// shutdown.
    #[cfg(test)]
    async fn next_event(&mut self) -> Event {
        self.events_rx.recv().await.unwrap()
    }

    #[rustfmt::skip]
    /// Handle event of main loop
    ///
    /// The events are attached to _exclusive_, _sharing_, or _concurrent_
    /// handlers.
    ///
    /// - Exclusive handlers are passed an `&mut` to the world state so they can
    ///   update it.
    /// - Sharing handlers are passed a simple reference. In principle we could
    ///   run these concurrently but we run these one handler at a time for simplicity.
    /// - When concurrent handlers are needed for performance reason (one tick
    ///   of the main loop should be as fast as possible to increase throughput)
    ///   they are spawned on blocking threads and provided a snapshot (clone) of
    ///   the state.
    async fn handle_event(&mut self, event: Event) -> anyhow::Result<()> {
        let loop_tick = std::time::Instant::now();

        // Diagnostics read the oak database (workspace symbols, imports,
        // resolved definitions), so any handler that writes to oak invalidates
        // them. Rather than have each write site remember to refresh, we watch
        // the oak revision across the whole tick: if a handler advanced it,
        // refresh centrally. Config and console state live outside oak, so
        // handlers that mutate those still refresh explicitly.
        let old_revision = salsa::plumbing::current_revision(&self.world.db);

        match event {
            Event::Lsp(msg) => match msg {
                LspMessage::Notification(notif) => {
                    lsp::log_info!("{notif:#?}");
                    lsp::log_info!(
                        "Entering notification handler with {n} outstanding Salsa db holds",
                        n = self.world.db.outstanding_holds()
                    );

                    match notif {
                        LspNotification::Initialized(_params) => {
                            handlers::handle_initialized(&self.client, &self.lsp_state).await?;
                        },
                        LspNotification::DidChangeWorkspaceFolders(params) => {
                            state_handlers::did_change_workspace_folders(params, &mut self.world, &mut self.lsp_state, &self.events_tx)?;
                        },
                        LspNotification::DidChangeConfiguration(params) => {
                            state_handlers::did_change_configuration(params, &self.client, &mut self.world).await?;
                        },
                        LspNotification::DidChangeWatchedFiles(params) => {
                            state_handlers::did_change_watched_files(params, &mut self.world, &mut self.lsp_state, &self.events_tx)?;
                        },
                        LspNotification::DidOpenTextDocument(params) => {
                            state_handlers::did_open(params, &mut self.world)?;
                        },
                        LspNotification::DidChangeTextDocument(params) => {
                            state_handlers::did_change(params, &mut self.lsp_state, &mut self.world)?;
                        },
                        LspNotification::DidSaveTextDocument(_params) => {
                            // Currently ignored
                        },
                        LspNotification::DidCloseTextDocument(params) => {
                            state_handlers::did_close(params, &mut self.world)?;
                        },
                    }
                },

                LspMessage::Request(request, tx) => {
                    lsp::log_info!("{request:#?}");

                    match request {
                        LspRequest::Initialize(params) => {
                            respond(tx, || state_handlers::initialize(params, &mut self.lsp_state, &mut self.world, &self.events_tx), LspResponse::Initialize)?;
                        },
                        LspRequest::WorkspaceSymbol(params) => {
                            respond(tx, || handlers::handle_symbol(params, &self.world), LspResponse::WorkspaceSymbol)?;
                        },
                        LspRequest::DocumentSymbol(params) => {
                            respond(tx, || handlers::handle_document_symbol(params, &self.world), LspResponse::DocumentSymbol)?;
                        },
                        LspRequest::FoldingRange(params) => {
                            respond(tx, || handlers::handle_folding_range(params, &self.world), LspResponse::FoldingRange)?;
                        },
                        LspRequest::ExecuteCommand(_params) => {
                            let response = handlers::handle_execute_command(&self.client).await;
                            respond(tx, || response, LspResponse::ExecuteCommand)?;
                        },
                        LspRequest::Completion(params) => {
                            respond(tx, || handlers::handle_completion(params, &self.world), LspResponse::Completion)?;
                        },
                        LspRequest::CompletionResolve(params) => {
                            respond(tx, || handlers::handle_completion_resolve(params), LspResponse::CompletionResolve)?;
                        },
                        LspRequest::Hover(params) => {
                            respond(tx, || handlers::handle_hover(params, &self.world), LspResponse::Hover)?;
                        },
                        LspRequest::SignatureHelp(params) => {
                            respond(tx, || handlers::handle_signature_help(params, &self.world), LspResponse::SignatureHelp)?;
                        },
                        LspRequest::GotoDefinition(params) => {
                            respond(tx, || handlers::handle_goto_definition(params, &self.world), LspResponse::GotoDefinition)?;
                        },
                        LspRequest::GotoImplementation(_params) => {
                            // TODO
                            respond(tx, || Ok(None), LspResponse::GotoImplementation)?;
                        },
                        LspRequest::SelectionRange(params) => {
                            respond(tx, || handlers::handle_selection_range(params, &self.world), LspResponse::SelectionRange)?;
                        },
                        LspRequest::References(params) => {
                            respond(tx, || handlers::handle_references(params, &self.world), LspResponse::References)?;
                        },
                        LspRequest::PrepareRename(params) => {
                            respond(tx, || handlers::handle_prepare_rename(params, &self.world), LspResponse::PrepareRename)?;
                        },
                        LspRequest::Rename(params) => {
                            respond(tx, || handlers::handle_rename(params, &self.world), LspResponse::Rename)?;
                        },
                        LspRequest::StatementRange(params) => {
                            respond(tx, || handlers::handle_statement_range(params, &self.world), LspResponse::StatementRange)?;
                        },
                        LspRequest::HelpTopic(params) => {
                            respond(tx, || handlers::handle_help_topic(params, &self.world), LspResponse::HelpTopic)?;
                        },
                        LspRequest::OnTypeFormatting(params) => {
                            state_handlers::did_change_formatting_options(&params.text_document_position.text_document.uri, &params.options, &mut self.world);
                            respond(tx, || handlers::handle_indent(params, &self.world), LspResponse::OnTypeFormatting)?;
                        },
                        LspRequest::CodeAction(params) => {
                            respond(tx, || handlers::handle_code_action(params, &self.lsp_state, &self.world), LspResponse::CodeAction)?;
                        },
                        LspRequest::VirtualDocument(params) => {
                            respond(tx, || handlers::handle_virtual_document(params, &self.world), LspResponse::VirtualDocument)?;
                        },
                        LspRequest::InputBoundaries(params) => {
                            respond(tx, || handlers::handle_input_boundaries(params), LspResponse::InputBoundaries)?;
                        },
                    };
                },
            },

            Event::Kernel(notif) => {
                lsp::log_info!("{notif:#?}", notif = notif.trace());

                match notif {
                    KernelNotification::DidChangeConsoleInputs(inputs) => {
                        state_handlers::did_change_console_inputs(inputs, &mut self.world)?;
                    },
                    KernelNotification::DidOpenVirtualDocument(params) => {
                        state_handlers::did_open_virtual_document(params, &mut self.world)?;
                    },
                    KernelNotification::DidCloseVirtualDocument(params) => {
                        state_handlers::did_close_virtual_document(params, &mut self.world)?
                    }
                }
            },

            Event::OakScanCompleted(scan) => {
                lsp::log_info!("Received `OakScanCompleted`");

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

                // Recompute editor-owned files at apply time, not at spawn
                // time: a buffer may have opened or closed since the scan
                // kicked off. The buffer-drain inside `apply_scan_completed` uses
                // this set as its watcher-event `skip` argument.
                let editor_owned: HashSet<FilePath> = self.world.open_files.keys().cloned().collect();
                let followups = self.lsp_state.oak_scheduler.apply_scan_completed(
                    &mut self.world.db,
                    scan,
                    &editor_owned,
                );
                lsp::log_info!(
                    "Dispatching {n} followup scan requests with {n_holds} outstanding Salsa db holds",
                    n = followups.len(),
                    n_holds = self.world.db.outstanding_holds(),
                );

                dispatch_scan_requests(&self.events_tx, followups);

                // Warm the workspace index once the scan settles. Editor
                // writes don't need to re-warm: they imply an open document,
                // and the diagnostics passes they trigger force the same
                // memos.
                if !self.lsp_state.oak_scheduler.has_pending_scans() {
                    warm_workspace_index(self.world.db.clone());
                }
            },

            Event::SourceCompleted(SourceCompleted { package, response }) => {
                if let Some(directory) = self.lsp_state.source_scheduler.finish(package, response) {
                    self.world.db.set_package_sources(package, &directory);
                }
            },
        }
        lsp::log_info!("Finished handling event in {}ms", loop_tick.elapsed().as_millis());

        // TODO: Make this threshold configurable by the client
        if loop_tick.elapsed() > std::time::Duration::from_millis(50) {
            lsp::log_info!("Handler took more than 50ms");
        }

        if salsa::plumbing::current_revision(&self.world.db) != old_revision {
            lsp::log_info!("World state revision advanced");
            diagnostics_refresh_all(&self.world);
            self.lsp_state
                .source_scheduler
                .schedule(&self.world.db, &self.events_tx);
        }

        Ok(())
    }

    #[allow(dead_code)] // Currently unused
    /// Spawn blocking thread for LSP request handler
    ///
    /// Use this for handlers that might take too long to handle on the main
    /// loop and negatively affect throughput.
    ///
    /// The LSP protocol allows concurrent handling as long as it doesn't affect
    /// correctness of responses. For instance handlers that only inspect the
    /// world state could be run concurrently. On the other hand, handlers that
    /// manipulate documents (e.g. formatting or refactoring) should not.
    fn spawn_handler<T, Handler>(
        response_tx: TokioUnboundedSender<RequestResponse>,
        handler: Handler,
        into_lsp_response: impl FnOnce(T) -> LspResponse + Send + 'static,
    ) where
        Handler: FnOnce() -> LspResult<T>,
        Handler: Send + 'static,
    {
        lsp::spawn_blocking(move || respond(response_tx, handler, into_lsp_response).and(Ok(None)))
    }
}

/// Build the LSP's [`SourceHandler`], or `None` to disable source fetching
fn source_handler(r_home: &Path) -> Option<Arc<dyn SourceHandler>> {
    if !cfg!(debug_assertions) {
        // TODO!: Remove this to activate in release builds as well.
        // Currently only active in debug builds (including unit and integration tests).
        return None;
    }

    let Some(r) = harp::command::r_executable(r_home) else {
        log::warn!(
            "Can't locate an R executable under '{}', package source fetching is disabled",
            r_home.display()
        );
        return None;
    };

    match OakSourceHandler::new(r) {
        Ok(handler) => Some(Arc::new(handler)),
        Err(err) => {
            log::error!(
                "Can't create package source handler, source fetching is disabled: {err:?}"
            );
            None
        },
    }
}

/// Test-only methods for driving the main loop without R or a live LSP
/// connection. Kept here, next to the loop they exercise, so the pump uses the
/// real `handle_event()` and the private channels rather than a reconstruction.
#[cfg(test)]
impl GlobalState {
    /// Run `event` through the real `handle_event`, then pump any pending
    /// events until we reach quiescence. This includes:
    /// - Pending oak scans
    /// - Pending source requests
    pub(crate) async fn handle_event_to_quiescence(&mut self, event: Event) {
        self.handle_event(event).await.unwrap();
        while self.lsp_state.oak_scheduler.has_pending_scans() ||
            self.lsp_state.source_scheduler.has_pending()
        {
            let event = self.next_event().await;
            self.handle_event(event).await.unwrap();
        }
    }

    pub(crate) fn world(&self) -> &WorldState {
        &self.world
    }
}

/// Spawn each [`ScanRequest`] on a blocking task. Each task runs the
/// pure-I/O [`ScanRequest::run`] and ships the [`ScanCompleted`] back
/// to the main loop as [`Event::OakScanCompleted`], where the scheduler
/// then applies it.
pub(super) fn dispatch_scan_requests(
    events_tx: &TokioUnboundedSender<Event>,
    requests: Vec<ScanRequest>,
) {
    for req in requests {
        let tx = events_tx.clone();
        spawn_blocking(move || {
            let scan = req.run();
            tx.send(Event::OakScanCompleted(scan)).log_err();
            Ok(None)
        });
    }
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
fn respond<T>(
    response_tx: TokioUnboundedSender<RequestResponse>,
    response: impl FnOnce() -> LspResult<T>,
    into_lsp_response: impl FnOnce(T) -> LspResponse,
) -> anyhow::Result<()> {
    let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(response)) {
        Ok(Ok(t)) => RequestResponse::Result(Ok(into_lsp_response(t))),
        Ok(Err(e)) => RequestResponse::Result(Err(e)),
        Err(err) if err.downcast_ref::<salsa::Cancelled>().is_some() => {
            // A salsa write cancelled an oak query while the handler ran.
            // Report `ContentModified` so the client knows the content moved
            // under us and re-requests.
            RequestResponse::Result(Err(LspError::JsonRpc(jsonrpc::Error::content_modified())))
        },
        Err(err) => {
            // Set global crash flag to disable the LSP
            LSP_HAS_CRASHED.store(true, Ordering::Release);

            let msg: String = if let Some(msg) = err.downcast_ref::<&str>() {
                msg.to_string()
            } else if let Some(msg) = err.downcast_ref::<String>() {
                msg.clone()
            } else {
                String::from("Couldn't retrieve the message.")
            };

            // This creates an uninformative backtrace that is reported in the
            // LSP logs. Note that the relevant backtrace is the one created by
            // our panic hook and reported via the _kernel_ logs.
            RequestResponse::Crashed(anyhow!("Panic occurred while handling request: {msg}"))
        },
    };

    let out = match response {
        RequestResponse::Result(Ok(_)) => Ok(()),
        RequestResponse::Result(Err(ref error)) => {
            // The error has already been sent to the client on `response_tx`
            // as a jsonrpc error, so the user sees the popup. Log here at
            // info level (with `{:?}` for the full debug format including a
            // backtrace) so server logs keep diagnostic context.
            lsp::log_info!("Error while handling request:\n{error:?}");
            Ok(())
        },
        RequestResponse::Crashed(ref error) => {
            Err(anyhow!("Crashed while handling request:\n{error:?}"))
        },
        RequestResponse::Disabled => Err(anyhow!("Received impossible `Disabled` response state")),
    };

    // Ignore errors from a closed channel. This indicates the request has
    // been cancelled on the tower-lsp side.
    let _ = response_tx.send(response);

    out
}

// Needed for spawning the loop
unsafe impl Sync for AuxiliaryState {}

impl AuxiliaryState {
    fn new(client: Client) -> Self {
        // Channels for communication with the auxiliary loop
        let (auxiliary_event_tx, auxiliary_event_rx) = tokio_unbounded_channel::<AuxiliaryEvent>();

        // Set global instance of this channel. This is used for interacting with the
        // auxiliary loop (logging messages or spawning a task) from free functions.
        // Unfortunately this can theoretically be reset at any time, i.e. on reconnection
        // after a refresh, which is why we need an RwLock. This is the only place we take
        // a write lock though. We panic if we can't access the write lock, as that implies
        // the auxiliary loop has gone down and something is very wrong. We hold the lock
        // for as short as possible, hence the extra scope.
        {
            let mut tx = AUXILIARY_EVENT_TX.write().unwrap();
            *tx = Some(auxiliary_event_tx);
        }

        // List of pending tasks for which we manage the lifecycle (mainly relay
        // errors and panics)
        let tasks = futures::stream::FuturesUnordered::new();

        // Prevent the stream from ever being empty so that `tasks.next()` never
        // resolves to `None`
        let pending =
            tokio::task::spawn(future::pending::<anyhow::Result<Option<AuxiliaryEvent>>>());
        let pending =
            Box::pin(pending) as Pin<Box<dyn AnyhowJoinHandleFut<Option<AuxiliaryEvent>> + Send>>;
        tasks.push(pending);

        Self {
            client,
            auxiliary_event_rx,
            tasks,
            published_diagnostics: HashMap::new(),
        }
    }

    /// Start the auxiliary loop
    ///
    /// Takes ownership of auxiliary state and start the low-latency auxiliary
    /// loop.
    async fn start(mut self) {
        loop {
            match self.next_event().await {
                AuxiliaryEvent::Log(level, message) => self.log(level, message).await,
                AuxiliaryEvent::SpawnedTask(handle) => self.tasks.push(Box::pin(handle)),
                AuxiliaryEvent::PublishDiagnostics(uri, diagnostics, version) => {
                    self.publish_diagnostics(uri, diagnostics, version).await
                },
                AuxiliaryEvent::Shutdown => break,
            }
        }
    }

    async fn next_event(&mut self) -> AuxiliaryEvent {
        loop {
            tokio::select! {
                event = self.auxiliary_event_rx.recv() => match event {
                    // Because of the way we communicate with the auxiliary loop
                    // via global state, the channel may become closed if a new
                    // LSP session is started in the process. This normally
                    // should not happen but for now we have to be defensive
                    // against this situation, see:
                    // https://github.com/posit-dev/ark/issues/622
                    // https://github.com/posit-dev/positron/issues/5321
                    Some(event) => return event,
                    None => return AuxiliaryEvent::Shutdown,
                },

                handle = self.tasks.next() => match handle.unwrap() {
                    // A joined task returned an event for us, handle it
                    Ok(Ok(Some(event))) => return event,

                    // Otherwise relay any errors and loop back into select
                    Err(err) => self.log_error(format!("A task panicked:\n{err:?}")).await,
                    Ok(Err(err)) => self.log_error(format!("A task failed:\n{err:?}")).await,
                    _ => (),
                },
            }
        }
    }

    /// Publish diagnostics for `uri`, skipping the client round-trip when the
    /// set is identical to what we last sent for that file. Only non-empty
    /// sets are remembered, and an absent entry counts as empty. So an empty
    /// result is published only when it clears diagnostics the client is
    /// currently showing, and the map stays bounded by the files on screen
    /// with diagnostics.
    async fn publish_diagnostics(
        &mut self,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    ) {
        let unchanged = match self.published_diagnostics.get(&uri) {
            Some(old) => diagnostics == *old,
            None => diagnostics.is_empty(),
        };
        if unchanged {
            return;
        }

        if diagnostics.is_empty() {
            self.published_diagnostics.remove(&uri);
        } else {
            self.published_diagnostics
                .insert(uri.clone(), diagnostics.clone());
        }

        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await
    }

    async fn log(&self, level: MessageType, message: String) {
        self.client.log_message(level, message).await
    }
    async fn log_error(&self, message: String) {
        self.client.log_message(MessageType::ERROR, message).await
    }
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

/// Initialise the auxiliary channel for unit tests that exercise LSP
/// handlers calling `publish_diagnostics` / `log` / similar.
///
/// Production wires the sender during `AuxiliaryState::start`. Tests don't
/// run that path, so `with_auxiliary_tx` would panic. This sets a sender
/// into the static and returns the receiver so tests can assert on events.
#[cfg(test)]
pub(crate) fn init_aux_for_test() -> TokioUnboundedReceiver<AuxiliaryEvent> {
    let (tx, rx) = tokio_unbounded_channel::<AuxiliaryEvent>();
    *AUXILIARY_EVENT_TX.write().unwrap() = Some(tx);
    rx
}

/// Send a message to the LSP client. This is non-blocking and treated on a
/// latency-sensitive task.
pub(crate) fn log(level: lsp_types::MessageType, message: String) {
    // We're not connected to an LSP client when running unit tests
    if cfg!(test) {
        return;
    }

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

/// Spawn a blocking task
///
/// This runs tasks that do semantic analysis on a separate thread pool to avoid
/// blocking the main loop.
///
/// Can optionally return an event for the auxiliary loop (i.e. a log message or
/// diagnostics publication).
///
/// Salsa cancellation is handled here so callers don't have to. A `set_*` on
/// the main loop cancels concurrent oak queries by unwinding with `Cancelled`.
/// We swallow that into `Ok(None)`, so a cancelled task is a quiet no-op
/// instead of a logged "task panicked". The write that cancelled it enqueues
/// its own follow-up. Any other panic still surfaces on join.
pub(crate) fn spawn_blocking<Handler>(handler: Handler)
where
    Handler: FnOnce() -> anyhow::Result<Option<AuxiliaryEvent>>,
    Handler: Send + 'static,
{
    let handle =
        tokio::task::spawn_blocking(move || catch_cancellation(handler).unwrap_or(Ok(None)));

    // Send the join handle to the auxiliary loop so it can log any errors
    // or panics
    send_auxiliary(AuxiliaryEvent::SpawnedTask(handle));
}

pub(crate) fn publish_diagnostics(uri: Url, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
    send_auxiliary(AuxiliaryEvent::PublishDiagnostics(
        uri,
        diagnostics,
        version,
    ));
}

impl KernelNotification {
    pub(crate) fn trace(&self) -> TraceKernelNotification<'_> {
        TraceKernelNotification { inner: self }
    }
}

impl std::fmt::Debug for TraceKernelNotification<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner {
            KernelNotification::DidChangeConsoleInputs(_) => f.write_str("DidChangeConsoleInputs"),
            KernelNotification::DidOpenVirtualDocument(params) => f
                .debug_struct("DidOpenVirtualDocument")
                .field("uri", &params.uri)
                .field("contents", &"<snip>")
                .finish(),
            KernelNotification::DidCloseVirtualDocument(params) => f
                .debug_struct("DidCloseVirtualDocument")
                .field("uri", &params.uri)
                .finish(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct RefreshDiagnosticsTask {
    /// Snapshot carrying the live oak plus the session context the diagnostics
    /// walk reads. See [`WorldState::diagnostics_snapshot`].
    state: WorldState,
    /// The file to diagnose, built against the live oak at enqueue time.
    file: OpenFile,
}

#[derive(Debug)]
struct RefreshDiagnosticsResult {
    uri: Url,
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
}

static DIAGNOSTICS_QUEUE: LazyLock<tokio::sync::mpsc::UnboundedSender<RefreshDiagnosticsTask>> =
    LazyLock::new(|| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(process_diagnostics_queue(rx));
        tx
    });

/// Process diagnostics refresh tasks.
///
/// Tasks are batched and deduplicated per URL (only the last task per URL is
/// processed), so stale-version diagnostics get superseded within a batch.
///
/// Batches triggered by an oak write can't publish out of order. Each pass
/// holds a db snapshot, and a salsa write blocks until all snapshots drop, so
/// by the time the write completes and the newer batch is enqueued, any older
/// pass has either unwound with `Cancelled` or already produced its result.
///
/// FIXME: Batches triggered without an oak write (console inputs, diagnostics
/// config) have no such barrier. An older pass can run concurrently with the
/// newer one and publish last, leaving diagnostics computed from the older
/// console scopes or config until the next refresh.
async fn process_diagnostics_queue(mut rx: mpsc::UnboundedReceiver<RefreshDiagnosticsTask>) {
    while let Some(task) = rx.recv().await {
        let mut batch = vec![task];
        while let Ok(task) = rx.try_recv() {
            batch.push(task);
        }
        process_diagnostics_batch(batch);
    }
    lsp::log_warn!("process_diagnostics_queue: channel closed, task exiting");
}

fn process_diagnostics_batch(batch: Vec<RefreshDiagnosticsTask>) {
    // Deduplicate tasks by keeping only the last one for each URI. We use a
    // `HashMap` so only the last insertion is retained. This is effectively a
    // way of cancelling diagnostics tasks for outdated documents.
    let batch: HashMap<_, _> = batch
        .into_iter()
        .map(|task| (task.file.wire_url().clone(), task))
        .collect();

    tracing::trace!("Processing {n} diagnostic tasks", n = batch.len());
    lsp::log_info!("Processing {n} diagnostic tasks", n = batch.len());

    // Each file is its own blocking task. `spawn_blocking()` catches salsa
    // cancellation, so a pass cancelled by a concurrent edit just produces no
    // event. The publish happens via the returned [`AuxiliaryEvent`].
    for (_uri, task) in batch {
        lsp::spawn_blocking(move || {
            let result = refresh_diagnostics(task);
            Ok(Some(AuxiliaryEvent::PublishDiagnostics(
                result.uri,
                result.diagnostics,
                result.version,
            )))
        });
    }
}

fn refresh_diagnostics(task: RefreshDiagnosticsTask) -> RefreshDiagnosticsResult {
    let RefreshDiagnosticsTask { file, state } = task;
    let uri = file.wire_url().clone();
    let version = file.version();
    let _span = tracing::info_span!("diagnostics_refresh", uri = %uri).entered();

    // Special case testthat-specific behaviour. This is a simple stopgap
    // approach that has some false positives (e.g. when we work on testthat
    // itself the flag will always be true), but that shouldn't have much
    // practical impact.
    let testthat = Path::new(uri.path())
        .components()
        .any(|c| c.as_os_str() == "testthat");

    let now = std::time::Instant::now();
    lsp::log_info!("Generating diagnostics for file: {uri}");

    let diagnostics = generate_diagnostics(file.file(), state, testthat);

    lsp::log_info!(
        "Finished diagnostics for file: {uri} in {:.0?}",
        now.elapsed()
    );

    RefreshDiagnosticsResult {
        uri,
        diagnostics,
        version,
    }
}

/// Run `f`, swallowing a salsa cancellation as `None`. Any other panic propagates.
fn catch_cancellation<T>(f: impl FnOnce() -> T) -> Option<T> {
    salsa::Cancelled::catch(std::panic::AssertUnwindSafe(f)).ok()
}

pub(crate) fn diagnostics_refresh_all(state: &WorldState) {
    tracing::trace!(
        "Refreshing diagnostics for {n} documents",
        n = state.open_files.len()
    );

    for file in state.open_files.values() {
        if !ExtUrl::should_diagnose(file.wire_url()) {
            continue;
        }

        DIAGNOSTICS_QUEUE
            .send(RefreshDiagnosticsTask {
                file: file.clone(),
                state: state.diagnostics_snapshot(),
            })
            .unwrap_or_else(|err| lsp::log_error!("Failed to queue diagnostics refresh: {err}"));
    }
}

/// Build the per-file workspace symbol indexes on a background thread so
/// main-loop consumers triggered by the user (workspace symbols, workspace
/// completions) find them already computed. The first run after a workspace
/// scan does the real work, parsing and walking each file. Later runs only
/// revalidate the per-file memos.
///
/// Mirrors rust-analyzer's cache warming: spawned when a workspace scan
/// settles, the analogue of r-a's transitions to quiescence (initial VFS scan,
/// workspace reload, etc). Unlike r-a we don't restart a warmup that gets
/// cancelled (`spawn_blocking()` swallows the unwind). A cancelling write can
/// only come from an editor buffer, so a document is open, and the diagnostics
/// passes spawned by that same write force the same memos and finish the job.
fn warm_workspace_index(db: OakDatabase) {
    spawn_blocking(move || {
        let now = std::time::Instant::now();
        lsp::log_info!("Starting workspace index warmup");
        indexer::warm(&db);
        lsp::log_info!("Finished workspace index warmup ({:.0?})", now.elapsed());
        Ok(None)
    })
}

#[cfg(test)]
mod tests {
    use aether_path::FilePath;
    use oak_scan::DbScan;
    use salsa::Database;
    use tower_lsp::jsonrpc;
    use url::Url;

    use super::catch_cancellation;
    use super::refresh_diagnostics;
    use super::respond;
    use super::tokio_unbounded_channel;
    use super::RefreshDiagnosticsTask;
    use crate::lsp::backend::LspError;
    use crate::lsp::backend::LspResponse;
    use crate::lsp::backend::RequestResponse;
    use crate::lsp::state::WorldState;

    /// A salsa cancellation during the pass is swallowed into `None` by
    /// `catch_cancellation`, the wrapper `spawn_blocking` applies to every task,
    /// rather than unwinding and killing the task.
    ///
    /// `cancellation_token().cancel()` arms local cancellation on the snapshot's
    /// oak, so the first salsa query in `generate_diagnostics` (the `tree_sitter`
    /// fetch) unwinds with `salsa::Cancelled`, the same payload a concurrent
    /// `set_*` produces. The unwind fires before any R, so no `r_task` here.
    #[test]
    fn test_cancelled_diagnostics_pass_is_caught() {
        let mut state = WorldState::default();
        let uri = Url::parse("file:///test.R").unwrap();
        let code = "foo";
        let file = state
            .db
            .upsert_editor(FilePath::from_url(&uri), code.to_string());
        state.insert_open_file(uri.clone(), file, None);

        let file = state.open_file(&uri).unwrap().clone();
        let snapshot = state.diagnostics_snapshot();
        snapshot.db.cancellation_token().cancel();

        let task = RefreshDiagnosticsTask {
            file,
            state: snapshot,
        };
        assert!(catch_cancellation(|| refresh_diagnostics(task)).is_none());
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
            .db
            .upsert_editor(FilePath::from_url(&uri), "foo".to_string());
        state.insert_open_file(uri.clone(), file, None);

        let file = state.open_file(&uri).unwrap().clone();
        let snapshot = state.diagnostics_snapshot();
        snapshot.db.cancellation_token().cancel();

        let (response_tx, mut response_rx) = tokio_unbounded_channel::<RequestResponse>();
        respond(
            response_tx,
            || {
                let _ = file.tree_sitter(&snapshot.db);
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
        let before = salsa::plumbing::current_revision(&state.db);
        state.db.upsert_editor(
            FilePath::from_url(&Url::parse("file:///a.R").unwrap()),
            "x <- 1".to_string(),
        );
        let after = salsa::plumbing::current_revision(&state.db);
        assert_ne!(before, after);
    }
}
