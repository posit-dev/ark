use std::path::Path;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use tower_lsp_server::ls_types::CompletionParams;
use tower_lsp_server::ls_types::DidChangeConfigurationParams;
use tower_lsp_server::ls_types::DidChangeWatchedFilesParams;
use tower_lsp_server::ls_types::FileChangeType;
use tower_lsp_server::ls_types::FileEvent;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::TextDocumentIdentifier;
use tower_lsp_server::ls_types::TextDocumentPositionParams;
use tower_lsp_server::ls_types::Uri;

use crate::backend::LspMessage;
use crate::backend::LspNotification;
use crate::backend::LspRequest;
use crate::backend::RequestResponse;
use crate::handlers::Handlers;
use crate::harness::editor::notifications::initialize_with;
use crate::main_loop::Event;

/// An `initialize` request opening `path` as the sole workspace folder, from a
/// client that answers `workspace/configuration` like Positron does.
///
/// Hand back the response receiver along with the event. The caller has to hold
/// it for the duration of the test: dropping it early makes `respond()`'s send
/// fail, which nextest then reports as a leak.
pub fn initialize<H: Handlers>(path: &Path) -> (Event<H>, UnboundedReceiver<RequestResponse<H>>) {
    initialize_with(folders(path), true, None)
}

/// A pull-capable [`initialize()`] request with `initialization_options`.
pub fn initialize_with_options<H: Handlers>(
    path: &Path,
    initialization_options: Value,
) -> (Event<H>, UnboundedReceiver<RequestResponse<H>>) {
    initialize_with(folders(path), true, Some(initialization_options))
}

/// An [`initialize()`] request from a client without `workspace/configuration`
/// support, using `initialization_options` for its global settings.
pub fn initialize_without_configuration<H: Handlers>(
    path: &Path,
    initialization_options: Option<Value>,
) -> (Event<H>, UnboundedReceiver<RequestResponse<H>>) {
    initialize_with(folders(path), false, initialization_options)
}

fn folders(path: &Path) -> Vec<Uri> {
    vec![Uri::from_file_path(path).unwrap()]
}

/// A `didChangeConfiguration` notification. The server ignores its `settings`
/// payload and pulls the registered settings again.
pub fn did_change_configuration<H: Handlers>() -> Event<H> {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeConfiguration(DidChangeConfigurationParams {
            settings: serde_json::Value::Null,
        }),
    ))
}

/// A `workspace/didChangeWatchedFiles` notification for one change at `path`.
pub fn did_change_watched_file<H: Handlers>(path: &Path, typ: FileChangeType) -> Event<H> {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeWatchedFiles(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: Uri::from_file_path(path).unwrap(),
                typ,
            }],
        }),
    ))
}

/// A `textDocument/completion` request at `position` in the open document at
/// `path`, invoked without a trigger character.
pub fn completion<H: Handlers>(
    path: &Path,
    position: Position,
) -> (Event<H>, UnboundedReceiver<RequestResponse<H>>) {
    let params = CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: Uri::from_file_path(path).unwrap(),
            },
            position,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let event = Event::Lsp(LspMessage::Request(LspRequest::Completion(params), tx));
    (event, rx)
}
