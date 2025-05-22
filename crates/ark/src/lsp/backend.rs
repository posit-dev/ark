//
// backend.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(deprecated)]

use std::sync::atomic::Ordering;
use std::sync::Arc;

use amalthea::comm::server_comm::ServerStartMessage;
use amalthea::comm::server_comm::ServerStartedMessage;
use amalthea::comm::ui_comm::ShowMessageParams as UiShowMessageParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use crossbeam::channel::Sender;
use serde_json::Value;
use stdext::result::ResultOrLog;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::unbounded_channel as tokio_unbounded_channel;
use tower_lsp::jsonrpc;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::request::GotoImplementationParams;
use tower_lsp::lsp_types::request::GotoImplementationResponse;
use tower_lsp::lsp_types::SelectionRange;
use tower_lsp::lsp_types::*;
use tower_lsp::Client;
use tower_lsp::LanguageServer;
use tower_lsp::LspService;
use tower_lsp::Server;

use super::main_loop::LSP_HAS_CRASHED;
use crate::interface::RMain;
use crate::lsp::handlers::VirtualDocumentParams;
use crate::lsp::handlers::VirtualDocumentResponse;
use crate::lsp::handlers::ARK_VDOC_REQUEST;
use crate::lsp::help_topic;
use crate::lsp::help_topic::HelpTopicParams;
use crate::lsp::help_topic::HelpTopicResponse;
use crate::lsp::input_boundaries;
use crate::lsp::input_boundaries::InputBoundariesParams;
use crate::lsp::input_boundaries::InputBoundariesResponse;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::statement_range;
use crate::lsp::statement_range::StatementRangeParams;
use crate::lsp::statement_range::StatementRangeResponse;
use crate::r_task;

// This enum is useful for two things. First it allows us to distinguish a
// normal request failure from a crash. In the latter case we send a
// notification to the client so the user knows the LSP has crashed.
//
// Once the LSP has crashed all requests respond with an error. This prevents
// any handler from running while we process the message to shut down the
// server. The `Disabled` enum variant is an indicator of this state. We could
// have just created an anyhow error passed through the `Result` variant but that
// would flood the LSP logs with irrelevant backtraces.
pub(crate) enum RequestResponse {
    Disabled,
    Crashed(anyhow::Result<LspResponse>),
    Result(anyhow::Result<LspResponse>),
}

// Based on https://stackoverflow.com/a/69324393/1725177
macro_rules! cast_response {
    ($self:expr, $target:expr, $pat:path) => {{
        match $target {
            RequestResponse::Result(Ok($pat(resp))) => Ok(resp),
            RequestResponse::Result(Err(err)) => Err(new_jsonrpc_error(format!("{err:?}"))),
            RequestResponse::Crashed(err) => {
                // Notify user that the LSP has crashed and is no longer active
                report_crash();

                // The backtrace is reported via `err` and eventually shows up
                // in the LSP logs on the client side
                let _ = $self.shutdown_tx.send(()).await;
                Err(new_jsonrpc_error(format!("{err:?}")))
            },
            RequestResponse::Disabled => Err(new_jsonrpc_error(String::from(
                "The LSP server has crashed and is now shut down!",
            ))),
            _ => panic!("Unexpected variant while casting to {}", stringify!($pat)),
        }
    }};
}

fn report_crash() {
    let user_message = concat!(
        "The R language server has crashed and has been disabled. ",
        "Smart features such as completions will no longer work in this session. ",
        "Please report this crash to https://github.com/posit-dev/positron/issues ",
        "with full logs (see https://positron.posit.co/troubleshooting.html#python-and-r-logs)."
    );

    r_task(|| {
        let event = UiFrontendEvent::ShowMessage(UiShowMessageParams {
            message: String::from(user_message),
        });

        let main = RMain::get();
        if let Some(ui_comm_tx) = main.get_ui_comm_tx() {
            ui_comm_tx.send_event(event);
        }
    });
}

#[derive(Debug)]
pub(crate) enum LspMessage {
    Notification(LspNotification),
    Request(LspRequest, TokioUnboundedSender<RequestResponse>),
}

#[derive(Debug)]
pub(crate) enum LspNotification {
    Initialized(InitializedParams),
    DidChangeWorkspaceFolders(DidChangeWorkspaceFoldersParams),
    DidChangeConfiguration(DidChangeConfigurationParams),
    DidChangeWatchedFiles(DidChangeWatchedFilesParams),
    DidOpenTextDocument(DidOpenTextDocumentParams),
    DidChangeTextDocument(DidChangeTextDocumentParams),
    DidSaveTextDocument(DidSaveTextDocumentParams),
    DidCloseTextDocument(DidCloseTextDocumentParams),
}

#[derive(Debug)]
pub(crate) enum LspRequest {
    Initialize(InitializeParams),
    WorkspaceSymbol(WorkspaceSymbolParams),
    DocumentSymbol(DocumentSymbolParams),
    ExecuteCommand(ExecuteCommandParams),
    Completion(CompletionParams),
    CompletionResolve(CompletionItem),
    Hover(HoverParams),
    SignatureHelp(SignatureHelpParams),
    GotoDefinition(GotoDefinitionParams),
    GotoImplementation(GotoImplementationParams),
    SelectionRange(SelectionRangeParams),
    References(ReferenceParams),
    StatementRange(StatementRangeParams),
    HelpTopic(HelpTopicParams),
    OnTypeFormatting(DocumentOnTypeFormattingParams),
    CodeAction(CodeActionParams),
    VirtualDocument(VirtualDocumentParams),
    InputBoundaries(InputBoundariesParams),
}

#[derive(Debug)]
pub(crate) enum LspResponse {
    Initialize(InitializeResult),
    WorkspaceSymbol(Option<Vec<SymbolInformation>>),
    DocumentSymbol(Option<DocumentSymbolResponse>),
    ExecuteCommand(Option<Value>),
    Completion(Option<CompletionResponse>),
    CompletionResolve(CompletionItem),
    Hover(Option<Hover>),
    SignatureHelp(Option<SignatureHelp>),
    GotoDefinition(Option<GotoDefinitionResponse>),
    GotoImplementation(Option<GotoImplementationResponse>),
    SelectionRange(Option<Vec<SelectionRange>>),
    References(Option<Vec<Location>>),
    StatementRange(Option<StatementRangeResponse>),
    HelpTopic(Option<HelpTopicResponse>),
    OnTypeFormatting(Option<Vec<TextEdit>>),
    CodeAction(Option<CodeActionResponse>),
    VirtualDocument(VirtualDocumentResponse),
    InputBoundaries(InputBoundariesResponse),
}

#[derive(Debug)]
struct Backend {
    /// Shutdown notifier used to unwind tower-lsp and disconnect from the
    /// client when an LSP handler panics.
    shutdown_tx: tokio::sync::mpsc::Sender<()>,

    /// Channel for communication with the main loop.
    events_tx: TokioUnboundedSender<Event>,

    /// Handle to main loop. Drop it to cancel the loop, all associated tasks,
    /// and drop all owned state.
    _main_loop: tokio::task::JoinSet<()>,
}

impl Backend {
    async fn request(&self, request: LspRequest) -> RequestResponse {
        if LSP_HAS_CRASHED.load(Ordering::Acquire) {
            return RequestResponse::Disabled;
        }

        let (response_tx, mut response_rx) = tokio_unbounded_channel::<RequestResponse>();

        // Relay request to main loop
        self.events_tx
            .send(Event::Lsp(LspMessage::Request(request, response_tx)))
            .unwrap();

        // Wait for response from main loop
        response_rx.recv().await.unwrap()
    }

    fn notify(&self, notif: LspNotification) {
        // Relay notification to main loop
        self.events_tx
            .send(Event::Lsp(LspMessage::Notification(notif)))
            .unwrap();
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        cast_response!(
            self,
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
    ) -> Result<Option<Vec<SymbolInformation>>> {
        cast_response!(
            self,
            self.request(LspRequest::WorkspaceSymbol(params)).await,
            LspResponse::WorkspaceSymbol
        )
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        cast_response!(
            self,
            self.request(LspRequest::DocumentSymbol(params)).await,
            LspResponse::DocumentSymbol
        )
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<Value>> {
        cast_response!(
            self,
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

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        cast_response!(
            self,
            self.request(LspRequest::Completion(params)).await,
            LspResponse::Completion
        )
    }

    async fn completion_resolve(&self, item: CompletionItem) -> Result<CompletionItem> {
        cast_response!(
            self,
            self.request(LspRequest::CompletionResolve(item)).await,
            LspResponse::CompletionResolve
        )
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        cast_response!(
            self,
            self.request(LspRequest::Hover(params)).await,
            LspResponse::Hover
        )
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        cast_response!(
            self,
            self.request(LspRequest::SignatureHelp(params)).await,
            LspResponse::SignatureHelp
        )
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        cast_response!(
            self,
            self.request(LspRequest::GotoDefinition(params)).await,
            LspResponse::GotoDefinition
        )
    }

    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        cast_response!(
            self,
            self.request(LspRequest::GotoImplementation(params)).await,
            LspResponse::GotoImplementation
        )
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        cast_response!(
            self,
            self.request(LspRequest::SelectionRange(params)).await,
            LspResponse::SelectionRange
        )
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        cast_response!(
            self,
            self.request(LspRequest::References(params)).await,
            LspResponse::References
        )
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        cast_response!(
            self,
            self.request(LspRequest::OnTypeFormatting(params)).await,
            LspResponse::OnTypeFormatting
        )
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        cast_response!(
            self,
            self.request(LspRequest::CodeAction(params)).await,
            LspResponse::CodeAction
        )
    }
}

// Custom methods for the backend.
//
// NOTE: Request / notification methods _must_ accept a params object,
// even for notifications that don't include any auxiliary data.
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
impl Backend {
    async fn statement_range(
        &self,
        params: StatementRangeParams,
    ) -> jsonrpc::Result<Option<StatementRangeResponse>> {
        cast_response!(
            self,
            self.request(LspRequest::StatementRange(params)).await,
            LspResponse::StatementRange
        )
    }

    async fn help_topic(
        &self,
        params: HelpTopicParams,
    ) -> jsonrpc::Result<Option<HelpTopicResponse>> {
        cast_response!(
            self,
            self.request(LspRequest::HelpTopic(params)).await,
            LspResponse::HelpTopic
        )
    }

    async fn virtual_document(
        &self,
        params: VirtualDocumentParams,
    ) -> tower_lsp::jsonrpc::Result<VirtualDocumentResponse> {
        cast_response!(
            self,
            self.request(LspRequest::VirtualDocument(params)).await,
            LspResponse::VirtualDocument
        )
    }

    async fn input_boundaries(
        &self,
        params: InputBoundariesParams,
    ) -> tower_lsp::jsonrpc::Result<InputBoundariesResponse> {
        cast_response!(
            self,
            self.request(LspRequest::InputBoundaries(params)).await,
            LspResponse::InputBoundaries
        )
    }

    async fn notification(&self, params: Option<Value>) {
        log::info!("Received Positron notification: {:?}", params);
    }
}

pub fn start_lsp(
    runtime: Arc<Runtime>,
    server_start: ServerStartMessage,
    server_started_tx: Sender<ServerStartedMessage>,
) {
    runtime.block_on(async {
        let ip_address = server_start.ip_address();

        // Binding to port `0` to allow the OS to allocate a port for us to bind to
        let listener = TcpListener::bind(format!("{ip_address}:0")).await.unwrap();

        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => {
                log::error!("LSP: Failed to bind to {ip_address}:0: {error}");
                return;
            },
        };

        // Get the OS allocated port
        let port = address.port();

        log::trace!("LSP: Thread starting at address {ip_address}:{port}.");

        // Send the port back to `Shell` and eventually out to the frontend so it can connect
        server_started_tx
            .send(ServerStartedMessage::new(port))
            .or_log_error("LSP: Can't send server started notification");

        log::trace!("LSP: Waiting for client");
        let (stream, address) = listener.accept().await.unwrap();
        log::trace!("LSP: Connected to client: '{address}'");
        let (read, write) = tokio::io::split(stream);

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

        let init = |client: Client| {
            let state = GlobalState::new(client);
            let events_tx = state.events_tx();

            // Start main loop and hold onto the handle that keeps it alive
            let main_loop = state.start();

            // Forward event channel along to `RMain`.
            // This also updates an outdated channel after a reconnect.
            // `RMain` should be initialized by now, since the caller of this
            // function waits to receive the init notification sent on
            // `kernel_init_rx`. Even if it isn't, this should be okay because
            // `r_task()` defensively blocks until its sender is initialized.
            r_task({
                let events_tx = events_tx.clone();
                move || {
                    RMain::with_mut(|main| main.set_lsp_channel(events_tx));
                }
            });

            Backend {
                shutdown_tx,
                events_tx,
                _main_loop: main_loop,
            }
        };

        let (service, socket) = LspService::build(init)
            .custom_method(
                statement_range::POSITRON_STATEMENT_RANGE_REQUEST,
                Backend::statement_range,
            )
            .custom_method(help_topic::POSITRON_HELP_TOPIC_REQUEST, Backend::help_topic)
            .custom_method(ARK_VDOC_REQUEST, Backend::virtual_document)
            // In principle this should probably be a Jupyter request
            .custom_method(
                input_boundaries::POSITRON_INPUT_BOUNDARIES_REQUEST,
                Backend::input_boundaries,
            )
            .custom_method("positron/notification", Backend::notification)
            .finish();

        let server = Server::new(read, write, socket);

        tokio::select! {
            _ = server.serve(service) => {
                log::trace!(
                    "LSP: Thread exiting gracefully after connection closed ({:?}).",
                    address
                );
            },
            _ = shutdown_rx.recv() => {
                log::trace!(
                    "LSP: Thread exiting after receiving a shutdown request ({:?}).",
                    address
                );
            }
        }

        // Remove the LSP channel on the way out, we can no longer handle any LSP updates
        // from `RMain`, at least until someone starts the LSP up again.
        r_task({
            move || {
                RMain::with_mut(|main| main.remove_lsp_channel());
            }
        });
    })
}

fn new_jsonrpc_error(message: String) -> jsonrpc::Error {
    jsonrpc::Error {
        code: jsonrpc::ErrorCode::ServerError(-1),
        message,
        data: None,
    }
}
