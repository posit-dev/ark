use std::path::Path;

use tokio::sync::mpsc::UnboundedReceiver;
use tower_lsp_server::ls_types::DidChangeTextDocumentParams;
use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::DidOpenTextDocumentParams;
use tower_lsp_server::ls_types::InitializeParams;
use tower_lsp_server::ls_types::InitializedParams;
use tower_lsp_server::ls_types::TextDocumentContentChangeEvent;
use tower_lsp_server::ls_types::TextDocumentItem;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::VersionedTextDocumentIdentifier;
use tower_lsp_server::ls_types::WorkspaceFolder;
use tower_lsp_server::ls_types::WorkspaceFoldersChangeEvent;

use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::backend::LspRequest;
use crate::lsp::backend::RequestResponse;
use crate::lsp::main_loop::Event;

/// An `initialize` request opening `path` as the sole workspace folder.
///
/// Hand back the response receiver along with the event. The caller has to hold
/// it for the duration of the test: dropping it early makes `respond()`'s send
/// fail, which nextest then reports as a leak.
pub(crate) fn initialize(path: &Path) -> (Event, UnboundedReceiver<RequestResponse>) {
    let params = InitializeParams {
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: Uri::from_file_path(path).unwrap(),
            name: String::new(),
        }]),
        ..Default::default()
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let event = Event::Lsp(LspMessage::Request(LspRequest::Initialize(params), tx));
    (event, rx)
}

/// An `initialized` notification that starts configuration resolution and releases deferred source fetching.
pub(crate) fn initialized() -> Event {
    Event::Lsp(LspMessage::Notification(LspNotification::Initialized(
        InitializedParams {},
    )))
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

/// A whole-document change at `version`, which must be greater than the version the
/// file was opened at.
pub(crate) fn did_change(path: &Path, contents: &str, version: i32) -> Event {
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

pub(crate) fn did_open(path: &Path, contents: &str) -> Event {
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
