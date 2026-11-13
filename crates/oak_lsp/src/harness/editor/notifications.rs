//! Editor notifications and requests represented as main-loop [`Event`]s.
//!
//! The session brings the server up with these, then drives documents through
//! the same handlers a real editor would. `tests::utils::events` builds
//! its own request variants on [`initialize_with()`].

use std::path::Path;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use tower_lsp_server::ls_types::ClientCapabilities;
use tower_lsp_server::ls_types::DidChangeTextDocumentParams;
use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::DidCloseTextDocumentParams;
use tower_lsp_server::ls_types::DidOpenTextDocumentParams;
use tower_lsp_server::ls_types::GotoDefinitionParams;
use tower_lsp_server::ls_types::InitializeParams;
use tower_lsp_server::ls_types::InitializedParams;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::TextDocumentContentChangeEvent;
use tower_lsp_server::ls_types::TextDocumentIdentifier;
use tower_lsp_server::ls_types::TextDocumentItem;
use tower_lsp_server::ls_types::TextDocumentPositionParams;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::VersionedTextDocumentIdentifier;
use tower_lsp_server::ls_types::WorkspaceClientCapabilities;
use tower_lsp_server::ls_types::WorkspaceFolder;
use tower_lsp_server::ls_types::WorkspaceFoldersChangeEvent;

use crate::backend::LspMessage;
use crate::backend::LspNotification;
use crate::backend::LspRequest;
use crate::backend::RequestResponse;
use crate::handlers::Handlers;
use crate::main_loop::Event;

/// An `initialize` request opening `folders` as the workspace, from a client
/// that answers `workspace/configuration` like Positron does.
pub(crate) fn initialize<H: Handlers>(
    folders: Vec<Uri>,
) -> (Event<H>, UnboundedReceiver<RequestResponse<H>>) {
    initialize_with(folders, true, None)
}

/// Hand back the response receiver along with the event. The caller has to
/// hold it until the main loop answers: dropping it early makes `respond()`'s
/// send fail, which nextest then reports as a leak.
pub(crate) fn initialize_with<H: Handlers>(
    folders: Vec<Uri>,
    configuration: bool,
    initialization_options: Option<Value>,
) -> (Event<H>, UnboundedReceiver<RequestResponse<H>>) {
    let params = InitializeParams {
        capabilities: ClientCapabilities {
            workspace: Some(WorkspaceClientCapabilities {
                configuration: Some(configuration),
                ..Default::default()
            }),
            ..Default::default()
        },
        workspace_folders: Some(
            folders
                .into_iter()
                .map(|uri| WorkspaceFolder {
                    uri,
                    name: String::new(),
                })
                .collect(),
        ),
        initialization_options,
        ..Default::default()
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let event = Event::Lsp(LspMessage::Request(LspRequest::Initialize(params), tx));
    (event, rx)
}

/// Load client configuration before releasing deferred source requests.
pub fn initialized<H: Handlers>() -> Event<H> {
    Event::Lsp(LspMessage::Notification(LspNotification::Initialized(
        InitializedParams {},
    )))
}

/// A whole-document change at `version`, which must be greater than the version the
/// file was opened at.
pub fn did_change<H: Handlers>(path: &Path, contents: &str, version: i32) -> Event<H> {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeTextDocument(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: Uri::from_file_path(path).unwrap(),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: contents.to_string(),
            }],
        }),
    ))
}

pub fn did_change_workspace_folders<H: Handlers>(path: &Path) -> Event<H> {
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

pub(crate) fn goto_definition<H: Handlers>(
    path: &Path,
    position: Position,
) -> (Event<H>, UnboundedReceiver<RequestResponse<H>>) {
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

pub(crate) fn did_close<H: Handlers>(path: &Path) -> Event<H> {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidCloseTextDocument(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: Uri::from_file_path(path).unwrap(),
            },
        }),
    ))
}

pub fn did_open<H: Handlers>(path: &Path, contents: &str) -> Event<H> {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidOpenTextDocument(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: Uri::from_file_path(path).unwrap(),
                language_id: String::from("r"),
                version: 0,
                text: contents.to_string(),
            },
        }),
    ))
}
