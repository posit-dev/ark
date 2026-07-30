use std::path::Path;

use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::DidOpenTextDocumentParams;
use tower_lsp_server::ls_types::TextDocumentItem;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::WorkspaceFolder;
use tower_lsp_server::ls_types::WorkspaceFoldersChangeEvent;

use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::main_loop::Event;

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
