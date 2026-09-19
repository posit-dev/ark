use std::path::Path;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use tower_lsp_server::ls_types::DidChangeConfigurationParams;
use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::GotoDefinitionParams;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::TextDocumentIdentifier;
use tower_lsp_server::ls_types::TextDocumentPositionParams;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::WorkspaceFolder;
use tower_lsp_server::ls_types::WorkspaceFoldersChangeEvent;

use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::backend::LspRequest;
use crate::lsp::backend::RequestResponse;
use crate::lsp::harness::editor::notifications::initialize_with;
use crate::lsp::main_loop::Event;

/// An `initialize` request opening `path` as the sole workspace folder, from a
/// client that answers `workspace/configuration` like Positron does.
///
/// Hand back the response receiver along with the event. The caller has to hold
/// it for the duration of the test: dropping it early makes `respond()`'s send
/// fail, which nextest then reports as a leak.
pub(crate) fn initialize(path: &Path) -> (Event, UnboundedReceiver<RequestResponse>) {
    initialize_with(folders(path), true, None)
}

/// A pull-capable [`initialize()`] request with `initialization_options`.
pub(crate) fn initialize_with_options(
    path: &Path,
    initialization_options: Value,
) -> (Event, UnboundedReceiver<RequestResponse>) {
    initialize_with(folders(path), true, Some(initialization_options))
}

/// An [`initialize()`] request from a client without `workspace/configuration`
/// support, using `initialization_options` for its global settings.
pub(crate) fn initialize_without_configuration(
    path: &Path,
    initialization_options: Option<Value>,
) -> (Event, UnboundedReceiver<RequestResponse>) {
    initialize_with(folders(path), false, initialization_options)
}

fn folders(path: &Path) -> Vec<Uri> {
    vec![Uri::from_file_path(path).unwrap()]
}

/// A `didChangeConfiguration` notification. The server ignores its `settings`
/// payload and pulls the registered settings again.
pub(crate) fn did_change_configuration() -> Event {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeConfiguration(DidChangeConfigurationParams {
            settings: serde_json::Value::Null,
        }),
    ))
}

pub(crate) fn did_change_workspace_folders(path: &Path) -> Event {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeWorkspaceFolders(DidChangeWorkspaceFoldersParams {
            event: WorkspaceFoldersChangeEvent {
                added: vec![WorkspaceFolder {
                    uri: Uri::from_file_path(path).unwrap(),
                    name: String::new(),
                }],
                removed: vec![],
            },
        }),
    ))
}

/// Create a `textDocument/definition` request and return its response receiver
/// so tests can assert the handler reply.
pub(crate) fn goto_definition(
    path: &Path,
    position: Position,
) -> (Event, UnboundedReceiver<RequestResponse>) {
    let params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: Uri::from_file_path(path).unwrap(),
            },
            position,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let event = Event::Lsp(LspMessage::Request(LspRequest::GotoDefinition(params), tx));
    (event, rx)
}
