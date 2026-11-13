//
// host.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Ark's [`Handlers`] implementation, R session inputs, and custom LSP methods.

mod requests;

use std::collections::HashMap;

use aether_path::FilePath;
pub(crate) use requests::VirtualDocumentParams;
pub(crate) use requests::VirtualDocumentResponse;
pub(crate) use requests::ARK_VDOC_REQUEST;
use serde_json::Value;
use stdext::result::ResultExt;
use tower_lsp_server::jsonrpc;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionParams;
use tower_lsp_server::ls_types::CompletionResponse;
use tower_lsp_server::ls_types::Hover;
use tower_lsp_server::ls_types::HoverParams;
use tower_lsp_server::ls_types::ServerInfo;
use tower_lsp_server::ls_types::SignatureHelp;
use tower_lsp_server::ls_types::SignatureHelpParams;
use tower_lsp_server::LspServiceBuilder;

use crate::console::ConsoleNotification;
use crate::lsp;
use crate::lsp::backend::new_jsonrpc_error;
use crate::lsp::backend::Backend;
use crate::lsp::backend::LspResult;
use crate::lsp::handlers::EventContext;
use crate::lsp::handlers::Handlers;
use crate::lsp::handlers::RequestContext;
use crate::lsp::help_topic;
use crate::lsp::help_topic::HelpTopicParams;
use crate::lsp::help_topic::HelpTopicResponse;
use crate::lsp::input_boundaries;
use crate::lsp::input_boundaries::InputBoundariesParams;
use crate::lsp::input_boundaries::InputBoundariesResponse;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::state::WorldState;
use crate::lsp::statement_range;
use crate::lsp::statement_range::StatementRangeParams;
use crate::lsp::statement_range::StatementRangeResponse;
use crate::url::FilePathExt;

/// Ark's LSP host. Completion, completion resolve, hover and signature help
/// consult the R session. Custom requests support Positron and Ark frontend
/// features.
#[derive(Debug)]
pub(crate) struct ArkHandlers {
    /// Notifies the console of document changes, so the DAP can invalidate
    /// breakpoints in edited documents.
    console_notification_tx: TokioUnboundedSender<ConsoleNotification>,

    /// Contents keyed by URI for `ark/internal/virtualDocument` requests.
    virtual_documents: HashMap<String, String>,
}

impl ArkHandlers {
    pub(crate) fn new(console_notification_tx: TokioUnboundedSender<ConsoleNotification>) -> Self {
        Self {
            console_notification_tx,
            virtual_documents: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ArkRequest {
    StatementRange(StatementRangeParams),
    HelpTopic(HelpTopicParams),
    VirtualDocument(VirtualDocumentParams),
    InputBoundaries(InputBoundariesParams),
}

#[derive(Debug)]
pub(crate) enum ArkResponse {
    StatementRange(Option<StatementRangeResponse>),
    HelpTopic(Option<HelpTopicResponse>),
    VirtualDocument(VirtualDocumentResponse),
    InputBoundaries(InputBoundariesResponse),
}

#[derive(Debug)]
pub(crate) enum ArkEvent {
    Kernel(KernelNotification),
    #[cfg(feature = "testing")]
    TestCancelRTask,
    #[cfg(feature = "testing")]
    TestPanicRTask,
    #[cfg(feature = "testing")]
    TestRErrorRTask,
}

/// Notifications from the console to the LSP.
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

/// Information sent from the kernel to the LSP after each top-level evaluation.
#[derive(Debug)]
pub struct ConsoleInputs {
    /// List of console scopes, from innermost (global or debug) to outermost
    /// scope. Currently the scopes are vectors of symbol names. TODO: In the
    /// future, we should send structural information like search path, and let
    /// the LSP query us for the contents so that the LSP can cache the
    /// information.
    pub console_scopes: Vec<Vec<String>>,
}

impl Handlers for ArkHandlers {
    type Request = ArkRequest;
    type Response = ArkResponse;
    type Event = ArkEvent;

    fn server_info(&self) -> ServerInfo {
        ServerInfo {
            name: "Ark R Kernel".to_string(),
            version: Some(crate::BUILD_VERSION.to_string()),
        }
    }

    fn completion(
        &self,
        params: CompletionParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<CompletionResponse>> {
        requests::handle_completion(params, cx)
    }

    fn completion_resolve(
        &self,
        item: CompletionItem,
        _cx: &RequestContext<'_>,
    ) -> LspResult<CompletionItem> {
        requests::handle_completion_resolve(item)
    }

    fn hover(&self, params: HoverParams, cx: &RequestContext<'_>) -> LspResult<Option<Hover>> {
        requests::handle_hover(params, cx.world)
    }

    fn signature_help(
        &self,
        params: SignatureHelpParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<SignatureHelp>> {
        requests::handle_signature_help(params, cx.world)
    }

    fn request(&self, request: ArkRequest, cx: &RequestContext<'_>) -> LspResult<ArkResponse> {
        match request {
            ArkRequest::StatementRange(params) => {
                requests::handle_statement_range(params, cx.world).map(ArkResponse::StatementRange)
            },
            ArkRequest::HelpTopic(params) => {
                requests::handle_help_topic(params, cx.world).map(ArkResponse::HelpTopic)
            },
            ArkRequest::VirtualDocument(params) => {
                requests::handle_virtual_document(params, &self.virtual_documents)
                    .map(ArkResponse::VirtualDocument)
            },
            ArkRequest::InputBoundaries(params) => {
                requests::handle_input_boundaries(params).map(ArkResponse::InputBoundaries)
            },
        }
    }

    fn event(&mut self, event: ArkEvent, cx: &mut EventContext<'_>) -> anyhow::Result<()> {
        match event {
            ArkEvent::Kernel(notif) => self.kernel_notification(notif, cx.world),

            #[cfg(feature = "testing")]
            ArkEvent::TestCancelRTask => {
                crate::r_task(|| {
                    std::panic::resume_unwind(Box::new(salsa::Cancelled::PendingWrite))
                });
                Ok(())
            },
            #[cfg(feature = "testing")]
            ArkEvent::TestPanicRTask => {
                crate::r_task(|| panic!("Test panic in an R task"));
                Ok(())
            },
            #[cfg(feature = "testing")]
            ArkEvent::TestRErrorRTask => {
                crate::r_task(|| -> () {
                    unsafe { libr::Rf_error(c"Test R error in an R task".as_ptr()) }
                });
                Ok(())
            },
        }
    }

    fn did_change_document(&mut self, path: &FilePath) {
        // Notify console about document change to invalidate breakpoints.
        self.console_notification_tx
            .send(ConsoleNotification::DidChangeDocument(path.clone()))
            .log_err();
    }

    fn should_diagnose(&self, path: &FilePath) -> bool {
        path.should_diagnose()
    }
}

impl ArkHandlers {
    fn kernel_notification(
        &mut self,
        notif: KernelNotification,
        state: &mut WorldState,
    ) -> anyhow::Result<()> {
        lsp::log_info!("{notif:#?}", notif = notif.trace());

        match notif {
            KernelNotification::DidChangeConsoleInputs(inputs) => {
                did_change_console_inputs(inputs, state)
            },
            KernelNotification::DidOpenVirtualDocument(params) => {
                self.did_open_virtual_document(params)
            },
            KernelNotification::DidCloseVirtualDocument(params) => {
                self.did_close_virtual_document(params)
            },
        }
    }

    #[tracing::instrument(level = "info", skip_all)]
    fn did_open_virtual_document(
        &mut self,
        params: DidOpenVirtualDocumentParams,
    ) -> anyhow::Result<()> {
        // Insert new document, replacing any old one
        self.virtual_documents.insert(params.uri, params.contents);
        Ok(())
    }

    #[tracing::instrument(level = "info", skip_all)]
    fn did_close_virtual_document(
        &mut self,
        params: DidCloseVirtualDocumentParams,
    ) -> anyhow::Result<()> {
        self.virtual_documents.remove(&params.uri);
        Ok(())
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change_console_inputs(
    inputs: ConsoleInputs,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    // Diagnostics use session names, including those exposed by
    // `devtools::load_all()` during package development, so scope changes
    // require a refresh.
    state.set_session_scopes(inputs.console_scopes);

    Ok(())
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

// Custom methods must accept a params object even when they carry no data.
// See the client parameter-serialization notes in `backend.rs`.

type ArkServiceBuilder = LspServiceBuilder<Backend<ArkHandlers>>;

/// Register Positron's and Ark's custom methods on the service.
pub(crate) fn custom_methods(builder: ArkServiceBuilder) -> ArkServiceBuilder {
    let builder = builder
        .custom_method(
            statement_range::POSITRON_STATEMENT_RANGE_REQUEST,
            statement_range,
        )
        .custom_method(help_topic::POSITRON_HELP_TOPIC_REQUEST, help_topic)
        .custom_method(ARK_VDOC_REQUEST, virtual_document)
        // In principle this should probably be a Jupyter request
        .custom_method(
            input_boundaries::POSITRON_INPUT_BOUNDARIES_REQUEST,
            input_boundaries,
        )
        .custom_method("positron/notification", notification);

    #[cfg(feature = "testing")]
    let builder = builder
        .custom_method(ARK_TEST_CANCEL_R_TASK, test_cancel_r_task)
        .custom_method(ARK_TEST_PANIC_R_TASK, test_panic_r_task)
        .custom_method(ARK_TEST_R_ERROR_R_TASK, test_r_error_r_task);

    builder
}

/// Extract the payload of `$pat`, or return a JSON-RPC error if the host
/// returned a different response variant.
macro_rules! cast_host_response {
    ($response:expr, $pat:path) => {{
        match $response? {
            $pat(response) => Ok(response),
            _ => {
                let message = format!("Unexpected variant while casting to {}", stringify!($pat));
                log::error!("{message}");
                Err(new_jsonrpc_error(message))
            },
        }
    }};
}

async fn statement_range(
    backend: &Backend<ArkHandlers>,
    params: StatementRangeParams,
) -> jsonrpc::Result<Option<StatementRangeResponse>> {
    cast_host_response!(
        backend
            .host_request(ArkRequest::StatementRange(params))
            .await,
        ArkResponse::StatementRange
    )
}

async fn help_topic(
    backend: &Backend<ArkHandlers>,
    params: HelpTopicParams,
) -> jsonrpc::Result<Option<HelpTopicResponse>> {
    cast_host_response!(
        backend.host_request(ArkRequest::HelpTopic(params)).await,
        ArkResponse::HelpTopic
    )
}

async fn virtual_document(
    backend: &Backend<ArkHandlers>,
    params: VirtualDocumentParams,
) -> jsonrpc::Result<VirtualDocumentResponse> {
    cast_host_response!(
        backend
            .host_request(ArkRequest::VirtualDocument(params))
            .await,
        ArkResponse::VirtualDocument
    )
}

async fn input_boundaries(
    backend: &Backend<ArkHandlers>,
    params: InputBoundariesParams,
) -> jsonrpc::Result<InputBoundariesResponse> {
    cast_host_response!(
        backend
            .host_request(ArkRequest::InputBoundaries(params))
            .await,
        ArkResponse::InputBoundaries
    )
}

async fn notification(_backend: &Backend<ArkHandlers>, params: Option<Value>) {
    log::info!("Received Positron notification: {:?}", params);
}

#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_CANCEL_R_TASK: &str = "ark/testCancelRTask";
#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_PANIC_R_TASK: &str = "ark/testPanicRTask";
#[cfg(feature = "testing")]
pub(crate) static ARK_TEST_R_ERROR_R_TASK: &str = "ark/testRErrorRTask";

#[cfg(feature = "testing")]
async fn test_cancel_r_task(backend: &Backend<ArkHandlers>, _params: Option<Value>) {
    backend.notify_host(ArkEvent::TestCancelRTask);
}

#[cfg(feature = "testing")]
async fn test_panic_r_task(backend: &Backend<ArkHandlers>, _params: Option<Value>) {
    backend.notify_host(ArkEvent::TestPanicRTask);
}

#[cfg(feature = "testing")]
async fn test_r_error_r_task(backend: &Backend<ArkHandlers>, _params: Option<Value>) {
    backend.notify_host(ArkEvent::TestRErrorRTask);
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use aether_path::FilePath;
    use oak_db::OakDatabase;
    use tower_lsp_server::ls_types::Diagnostic;

    use super::ArkEvent;
    use super::ArkHandlers;
    use super::ConsoleInputs;
    use super::KernelNotification;
    use crate::console::ConsoleNotification;
    use crate::lsp::harness::LspHarness;
    use crate::lsp::harness::LspSession;
    use crate::lsp::main_loop::Event;

    /// Bounds every wait so a lost wakeup fails here rather than hanging to the
    /// harness timeout.
    const TIMEOUT: Duration = Duration::from_secs(10);

    /// Console inputs are not oak inputs, so `set_session_scopes()` must bump
    /// the revision to refresh diagnostics for an unchanged document. The
    /// diagnostic for `foo` disappears when the session defines it and returns
    /// when the session removes it.
    #[tokio::test]
    async fn test_console_inputs_refresh_diagnostics() {
        let workspace = tempfile::tempdir().unwrap();
        let script = workspace.path().join("script.R");
        let (console_tx, _console_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = LspHarness::new(OakDatabase::new())
            .start_with_handlers(ArkHandlers::new(console_tx), &[])
            .await;
        let undefined = vec![String::from("No symbol named 'foo' in scope.")];

        session.send_did_open(&script, "foo\n");
        assert_eq!(next_messages(&mut session, &script).await, undefined);

        session.enqueue(console_inputs(vec![vec![String::from("foo")]]));
        assert_eq!(
            next_messages(&mut session, &script).await,
            Vec::<String>::new()
        );

        session.enqueue(console_inputs(vec![]));
        assert_eq!(next_messages(&mut session, &script).await, undefined);
    }

    /// The DAP invalidates breakpoints in a document when the console hears
    /// that the editor changed it.
    #[tokio::test]
    async fn test_document_change_notifies_the_console() {
        let workspace = tempfile::tempdir().unwrap();
        let script = workspace.path().join("script.R");
        let (console_tx, mut console_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = LspHarness::new(OakDatabase::new())
            .start_with_handlers(ArkHandlers::new(console_tx), &[])
            .await;

        session.send_did_open(&script, "x <- 1\n");
        session.send_did_change(&script, "x <- 2\n", 1);
        tokio::time::timeout(TIMEOUT, session.settle())
            .await
            .unwrap();

        let Ok(ConsoleNotification::DidChangeDocument(path)) = console_rx.try_recv() else {
            panic!("Expected a document change notification");
        };
        assert_eq!(path, FilePath::from_path_buf(script).unwrap());
        assert!(console_rx.try_recv().is_err());
    }

    fn console_inputs(console_scopes: Vec<Vec<String>>) -> Event<ArkHandlers> {
        Event::Host(ArkEvent::Kernel(
            KernelNotification::DidChangeConsoleInputs(ConsoleInputs { console_scopes }),
        ))
    }

    /// Wait for accepted diagnostics for `path` at the version sent by `didOpen`.
    async fn next_messages(session: &mut LspSession<ArkHandlers>, path: &Path) -> Vec<String> {
        let diagnostics: Vec<Diagnostic> =
            tokio::time::timeout(TIMEOUT, session.wait_for_accepted_diagnostics(path, 0))
                .await
                .unwrap();
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }
}
