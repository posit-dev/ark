//
// backend.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

#![allow(deprecated)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::task::Context as TaskContext;
use std::task::Poll;

use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc::unbounded_channel as tokio_unbounded_channel;
use tower::Service;
use tower_lsp_server::jsonrpc;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::request::GotoImplementationParams;
use tower_lsp_server::ls_types::request::GotoImplementationResponse;
use tower_lsp_server::ls_types::FoldingRange;
use tower_lsp_server::ls_types::SelectionRange;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::LspService;
use tower_lsp_server::LspServiceBuilder;
use tower_lsp_server::Server;

use super::main_loop::LspServiceContext;
use crate::handlers::Handlers;
#[cfg(feature = "testing")]
use crate::main_loop::panic_auxiliary_loop;
use crate::main_loop::report_crash;
use crate::main_loop::Event;
use crate::main_loop::GlobalState;
use crate::main_loop::LoopHandles;
use crate::main_loop::TokioUnboundedSender;
use crate::panic;
use crate::panic::Recovery;

// Once the LSP has crashed all requests respond with an error. This prevents
// any handler from running while we process the message to shut down the
// server. The `Disabled` enum variant is an indicator of this state. We could
// have just created an anyhow error passed through the `Result` variant but that
// would flood the LSP logs with irrelevant backtraces.
#[expect(clippy::large_enum_variant)]
pub enum RequestResponse<H: Handlers> {
    Disabled,
    Result(LspResult<LspResponse<H>>),
}

// Based on https://stackoverflow.com/a/69324393/1725177
macro_rules! cast_response {
    ($target:expr, $pat:path) => {{
        match $target {
            RequestResponse::Result(Ok($pat(resp))) => Ok(resp),
            RequestResponse::Result(Ok(_)) => {
                let message = format!("Unexpected variant while casting to {}", stringify!($pat));
                log::error!("{message}");
                Err(new_jsonrpc_error(message))
            },
            RequestResponse::Result(Err(err)) => match err {
                LspError::JsonRpc(err) => Err(err),
                // `{err}` (Display) prints just the message. `{err:?}` (Debug)
                // would include the captured backtrace when `RUST_BACKTRACE`
                // is set, which then surfaces in the client's error popup.
                LspError::Anyhow(err) => Err(new_jsonrpc_error(format!("{err}"))),
            },
            RequestResponse::Disabled => Err(new_jsonrpc_error(String::from(
                "The LSP server has crashed and is now shut down!",
            ))),
        }
    }};
}

#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
pub enum LspMessage<H: Handlers> {
    Notification(LspNotification),
    Request(LspRequest<H>, TokioUnboundedSender<RequestResponse<H>>),
}

#[derive(Debug)]
pub enum LspNotification {
    Initialized(InitializedParams),
    DidChangeWorkspaceFolders(DidChangeWorkspaceFoldersParams),
    DidChangeConfiguration(DidChangeConfigurationParams),
    DidChangeWatchedFiles(DidChangeWatchedFilesParams),
    DidOpenTextDocument(DidOpenTextDocumentParams),
    DidChangeTextDocument(DidChangeTextDocumentParams),
    DidSaveTextDocument(DidSaveTextDocumentParams),
    DidCloseTextDocument(DidCloseTextDocumentParams),
    DidOpenNotebookDocument(DidOpenNotebookDocumentParams),
    DidChangeNotebookDocument(DidChangeNotebookDocumentParams),
    DidCloseNotebookDocument(DidCloseNotebookDocumentParams),
    #[cfg(feature = "testing")]
    TestPanic,
}

#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
pub enum LspRequest<H: Handlers> {
    Initialize(InitializeParams),
    WorkspaceSymbol(WorkspaceSymbolParams),
    DocumentSymbol(DocumentSymbolParams),
    FoldingRange(FoldingRangeParams),
    ExecuteCommand(ExecuteCommandParams),
    Completion(CompletionParams),
    CompletionResolve(CompletionItem),
    Hover(HoverParams),
    SignatureHelp(SignatureHelpParams),
    GotoDefinition(GotoDefinitionParams),
    GotoImplementation(GotoImplementationParams),
    SelectionRange(SelectionRangeParams),
    References(ReferenceParams),
    PrepareRename(TextDocumentPositionParams),
    Rename(RenameParams),
    OnTypeFormatting(DocumentOnTypeFormattingParams),
    CodeAction(CodeActionParams),
    /// A request only the host serves, sent by one of its custom methods.
    Host(H::Request),
    #[cfg(feature = "testing")]
    TestPanic,
}

#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
pub enum LspResponse<H: Handlers> {
    Initialize(InitializeResult),
    WorkspaceSymbol(Option<Vec<SymbolInformation>>),
    DocumentSymbol(Option<DocumentSymbolResponse>),
    FoldingRange(Option<Vec<FoldingRange>>),
    ExecuteCommand(Option<Value>),
    Completion(Option<CompletionResponse>),
    CompletionResolve(CompletionItem),
    Hover(Option<Hover>),
    SignatureHelp(Option<SignatureHelp>),
    GotoDefinition(Option<GotoDefinitionResponse>),
    GotoImplementation(Option<GotoImplementationResponse>),
    SelectionRange(Option<Vec<SelectionRange>>),
    References(Option<Vec<Location>>),
    PrepareRename(Option<PrepareRenameResponse>),
    Rename(Option<WorkspaceEdit>),
    OnTypeFormatting(Option<Vec<TextEdit>>),
    CodeAction(Option<CodeActionResponse>),
    Host(H::Response),
    #[cfg(feature = "testing")]
    TestPanic(()),
}

pub type LspResult<T> = std::result::Result<T, LspError>;

#[derive(Debug)]
pub enum LspError {
    JsonRpc(jsonrpc::Error),
    Anyhow(anyhow::Error),
}

impl std::error::Error for LspError {}

impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LspError::JsonRpc(error) => write!(f, "{error:?}"),
            LspError::Anyhow(error) => write!(f, "{error:?}"),
        }
    }
}

// For the ability to `?` a `jsonrpc::Error` into an `LspError`
impl From<jsonrpc::Error> for LspError {
    fn from(error: jsonrpc::Error) -> Self {
        Self::JsonRpc(error)
    }
}

// For the ability to `?` an `anyhow::Error` into an `LspError`
impl From<anyhow::Error> for LspError {
    fn from(error: anyhow::Error) -> Self {
        Self::Anyhow(error)
    }
}

/// The `tower-lsp` service that relays analysis requests and editor notifications
/// to the main loop. Host custom methods use [`Self::host_request()`] and
/// [`Self::notify_host()`] to reach the same loop.
#[derive(Debug)]
pub struct Backend<H: Handlers> {
    /// Channel for communication with the main loop.
    events_tx: TokioUnboundedSender<Event<H>>,

    /// State shared with the main loop and its background pools.
    service_context: Arc<LspServiceContext>,

    /// Handle to the LSP loops. Drop it to shut the loops down and drop all
    /// owned state.
    _main_loop: LoopHandles,
}

impl<H: Handlers> Backend<H> {
    /// Relay a host-only request to the main loop, which answers it through
    /// [`Handlers::request()`].
    pub async fn host_request(&self, request: H::Request) -> Result<H::Response> {
        cast_response!(
            self.request(LspRequest::Host(request)).await,
            LspResponse::Host
        )
    }

    /// Relay a host event through the same channel as client requests and
    /// notifications. The main loop handles events serially in channel order.
    pub fn notify_host(&self, event: H::Event) {
        self.send(Event::Host(event));
    }

    async fn request(&self, request: LspRequest<H>) -> RequestResponse<H> {
        if self.service_context.has_crashed() {
            return RequestResponse::Disabled;
        }

        let (response_tx, mut response_rx) = tokio_unbounded_channel::<RequestResponse<H>>();

        // Relay request to main loop
        if self
            .events_tx
            .send(Event::Lsp(LspMessage::Request(request, response_tx)))
            .is_err()
        {
            return RequestResponse::Disabled;
        }

        // Wait for response from main loop
        match response_rx.recv().await {
            Some(response) => response,
            None => RequestResponse::Disabled,
        }
    }

    fn notify(&self, notif: LspNotification) {
        self.send(Event::Lsp(LspMessage::Notification(notif)));
    }

    fn send(&self, event: Event<H>) {
        if self.service_context.has_crashed() {
            return;
        }

        // Relay notification to main loop
        if self.events_tx.send(event).is_err() {
            log::error!("Can't relay notification, the main loop is gone");
        }
    }
}

struct CatchServicePanics<S> {
    inner: S,
    client: Client,
    service_context: Arc<LspServiceContext>,
}

impl<S> CatchServicePanics<S> {
    fn new(inner: S, client: Client, service_context: Arc<LspServiceContext>) -> Self {
        Self {
            inner,
            client,
            service_context,
        }
    }
}

impl<S> Service<jsonrpc::Request> for CatchServicePanics<S>
where
    S: Service<jsonrpc::Request, Response = Option<jsonrpc::Response>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::result::Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: jsonrpc::Request) -> Self::Future {
        let future = self.inner.call(request);
        let client = self.client.clone();
        let service_context = Arc::clone(&self.service_context);

        Box::pin(async move {
            match panic::catch_unwind_async_payload(Recovery::Always, future).await {
                Ok(response) => response,
                Err(payload) => {
                    report_and_resume_service_panic(client, service_context, payload).await
                },
            }
        })
    }
}

async fn report_and_resume_service_panic(
    client: Client,
    service_context: Arc<LspServiceContext>,
    payload: panic::PanicPayload,
) -> ! {
    let message = panic::message(&payload);
    log::error!("LSP: Panic in the service: {message}");
    service_context.set_crashed();
    report_crash(&client).await;
    std::panic::resume_unwind(payload);
}

impl<H: Handlers> LanguageServer for Backend<H> {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        cast_response!(
            self.request(LspRequest::Initialize(params)).await,
            LspResponse::Initialize
        )
    }

    async fn initialized(&self, params: InitializedParams) {
        self.notify(LspNotification::Initialized(params));
    }

    async fn shutdown(&self) -> Result<()> {
        // Don't go through the main loop because we want this request to
        // succeed even when the LSP has crashed and has been disabled.
        Ok(())
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        self.notify(LspNotification::DidChangeWorkspaceFolders(params));
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        self.notify(LspNotification::DidChangeConfiguration(params));
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        self.notify(LspNotification::DidChangeWatchedFiles(params));
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let info: Option<Vec<SymbolInformation>> = cast_response!(
            self.request(LspRequest::WorkspaceSymbol(params)).await,
            LspResponse::WorkspaceSymbol
        )?;
        Ok(info.map(WorkspaceSymbolResponse::Flat))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        cast_response!(
            self.request(LspRequest::DocumentSymbol(params)).await,
            LspResponse::DocumentSymbol
        )
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        cast_response!(
            self.request(LspRequest::FoldingRange(params)).await,
            LspResponse::FoldingRange
        )
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<Value>> {
        cast_response!(
            self.request(LspRequest::ExecuteCommand(params)).await,
            LspResponse::ExecuteCommand
        )
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.notify(LspNotification::DidOpenTextDocument(params));
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.notify(LspNotification::DidChangeTextDocument(params));
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.notify(LspNotification::DidSaveTextDocument(params));
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.notify(LspNotification::DidCloseTextDocument(params));
    }

    async fn notebook_did_open(&self, params: DidOpenNotebookDocumentParams) {
        self.notify(LspNotification::DidOpenNotebookDocument(params));
    }

    async fn notebook_did_change(&self, params: DidChangeNotebookDocumentParams) {
        self.notify(LspNotification::DidChangeNotebookDocument(params));
    }

    async fn notebook_did_close(&self, params: DidCloseNotebookDocumentParams) {
        self.notify(LspNotification::DidCloseNotebookDocument(params));
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        cast_response!(
            self.request(LspRequest::Completion(params)).await,
            LspResponse::Completion
        )
    }

    async fn completion_resolve(&self, item: CompletionItem) -> Result<CompletionItem> {
        cast_response!(
            self.request(LspRequest::CompletionResolve(item)).await,
            LspResponse::CompletionResolve
        )
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        cast_response!(
            self.request(LspRequest::Hover(params)).await,
            LspResponse::Hover
        )
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        cast_response!(
            self.request(LspRequest::SignatureHelp(params)).await,
            LspResponse::SignatureHelp
        )
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        cast_response!(
            self.request(LspRequest::GotoDefinition(params)).await,
            LspResponse::GotoDefinition
        )
    }

    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        cast_response!(
            self.request(LspRequest::GotoImplementation(params)).await,
            LspResponse::GotoImplementation
        )
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        cast_response!(
            self.request(LspRequest::SelectionRange(params)).await,
            LspResponse::SelectionRange
        )
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        cast_response!(
            self.request(LspRequest::References(params)).await,
            LspResponse::References
        )
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        cast_response!(
            self.request(LspRequest::PrepareRename(params)).await,
            LspResponse::PrepareRename
        )
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        cast_response!(
            self.request(LspRequest::Rename(params)).await,
            LspResponse::Rename
        )
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        cast_response!(
            self.request(LspRequest::OnTypeFormatting(params)).await,
            LspResponse::OnTypeFormatting
        )
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        cast_response!(
            self.request(LspRequest::CodeAction(params)).await,
            LspResponse::CodeAction
        )
    }
}

// Custom test methods that exercise the core's recovery boundaries.
//
// NOTE: Request / notification methods _must_ accept a params object,
// even for notifications that don't include any auxiliary data. This holds
// for the host's custom methods too.
//
// I'm not positive, but I think this is related to the way VSCode
// serializes parameters for notifications / requests when no data
// is supplied. Instead of supplying "nothing", it supplies something
// like `[null]` which tower_lsp seems to quietly reject when attempting
// to invoke the registered method.
//
// See also:
//
// https://github.com/Microsoft/vscode-languageserver-node/blob/18fad46b0e8085bb72e1b76f9ea23a379569231a/client/src/common/client.ts#L802-L838
// https://github.com/Microsoft/vscode-languageserver-node/blob/18fad46b0e8085bb72e1b76f9ea23a379569231a/client/src/common/client.ts#L701-L752
#[cfg(feature = "testing")]
impl<H: Handlers> Backend<H> {
    async fn test_panic(&self, _params: Option<Value>) -> jsonrpc::Result<()> {
        cast_response!(
            self.request(LspRequest::TestPanic).await,
            LspResponse::TestPanic
        )
    }

    async fn test_panic_notification(&self, _params: Option<Value>) {
        self.notify(LspNotification::TestPanic);
    }

    async fn test_panic_main_loop(&self, _params: Option<Value>) {
        let _ = self.events_tx.send(Event::TestPanicMainLoop);
    }

    // Bypass the main loop so the test event panics in the auxiliary loop.
    async fn test_panic_auxiliary(&self, _params: Option<Value>) {
        panic_auxiliary_loop();
    }

    // Bypass the main loop and panic directly in a `tower-lsp` service future.
    async fn test_panic_service(&self, _params: Option<Value>) {
        panic!("Test panic in the LSP service");
    }
}

#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_PANIC_REQUEST: &str = "ark/testPanic";
#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_PANIC_NOTIFICATION: &str = "ark/testPanicNotification";
#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_PANIC_MAIN_LOOP: &str = "ark/testPanicMainLoop";
#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_PANIC_AUXILIARY: &str = "ark/testPanicAuxiliary";
#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_PANIC_SERVICE: &str = "ark/testPanicService";

/// Serve the LSP over `read` and `write` until the connection closes or the
/// main loop shuts the server down after a crash.
///
/// `handlers` handles requests and host events. Supplying `r_executable`
/// enables source fetching, the sidecar job and R library resolution.
/// `on_start()` receives the event channel after the main loop is started,
/// so the host can post events. `custom_methods()` registers host methods
/// that can use [`Backend::host_request()`] and [`Backend::notify_host()`].
pub async fn serve<H: Handlers>(
    read: impl AsyncRead + Unpin,
    write: impl AsyncWrite,
    handlers: H,
    r_executable: Option<PathBuf>,
    on_start: impl FnOnce(&TokioUnboundedSender<Event<H>>),
    custom_methods: impl FnOnce(LspServiceBuilder<Backend<H>>) -> LspServiceBuilder<Backend<H>>,
) {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let crash_client = OnceLock::<Client>::new();
    let crash_service_context = OnceLock::<Arc<LspServiceContext>>::new();

    let init = |client: Client| {
        let _ = crash_client.set(client.clone());
        let state = GlobalState::new(handlers, client, r_executable);
        let events_tx = state.events_tx();
        let service_context = Arc::clone(state.service_context());
        let _ = crash_service_context.set(Arc::clone(&service_context));

        // Start main loop and hold onto the handle that keeps it alive
        let main_loop = state.start(shutdown_tx);

        on_start(&events_tx);

        Backend {
            events_tx,
            service_context,
            _main_loop: main_loop,
        }
    };

    let builder = custom_methods(LspService::build(init));

    #[cfg(feature = "testing")]
    let builder = builder
        .custom_method(ARK_TEST_PANIC_REQUEST, Backend::test_panic)
        .custom_method(
            ARK_TEST_PANIC_NOTIFICATION,
            Backend::test_panic_notification,
        )
        .custom_method(ARK_TEST_PANIC_MAIN_LOOP, Backend::test_panic_main_loop)
        .custom_method(ARK_TEST_PANIC_AUXILIARY, Backend::test_panic_auxiliary)
        .custom_method(ARK_TEST_PANIC_SERVICE, Backend::test_panic_service);

    let (service, socket) = builder.finish();
    let Some(client) = crash_client.into_inner() else {
        log::error!("LSP: Service did not initialize its client");
        return;
    };
    let Some(service_context) = crash_service_context.into_inner() else {
        log::error!("LSP: Service did not initialize its context");
        return;
    };
    let service = CatchServicePanics::new(service, client.clone(), service_context);

    // Finite transport concurrency and its pending queue can stop the socket reader while
    // handlers await the serial main loop, which can await a client configuration response.
    // Unlimited admission keeps the reader progressing without parallelizing main-loop handlers.
    let server = Server::new(read, write, socket).concurrency_level(usize::MAX);

    let serving = async {
        tokio::select! {
            _ = server.serve(service) => {
                log::trace!("LSP: Thread exiting gracefully after connection closed.");
            },
            _ = shutdown_rx.recv() => {
                log::trace!("LSP: Thread exiting after receiving a shutdown request.");
            }
        }
    };

    // Catch handler panics around `server.serve()`, which polls every `tower-lsp`
    // handler.
    if let Err(msg) = panic::catch_unwind_async(Recovery::Always, serving).await {
        log::error!("LSP: Panic in the service: {msg}");
    }
}

pub fn new_jsonrpc_error(message: String) -> jsonrpc::Error {
    jsonrpc::Error {
        code: jsonrpc::ErrorCode::ServerError(-1),
        message: message.into(),
        data: None,
    }
}
