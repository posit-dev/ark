// 
// backend.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use walkdir::WalkDir;

use crate::lsp::completions::append_document_completions;
use crate::lsp::document::Document;
use crate::lsp::indexer::WorkspaceIndexer;
use crate::lsp::logger::log_push;
use crate::lsp::macros::unwrap;

macro_rules! backend_trace {

    ($self: expr, $($rest: expr),*) => {{
        let message = format!($($rest, )*);
        $self.client.log_message(tower_lsp::lsp_types::MessageType::INFO, message).await
    }};

}

#[derive(Debug)]
pub(crate) struct Workspace {
    pub folders: Vec<Url>,
}

impl Default for Workspace {

    fn default() -> Self {
        Self { folders: Default::default() }
    }

}

#[derive(Debug)]
pub(crate) struct Backend {
    pub client: Client,
    pub documents: DashMap<Url, Document>,
    pub indexer: Arc<Mutex<WorkspaceIndexer>>,
    pub workspace: Arc<Mutex<Workspace>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        backend_trace!(self, "initialize({:#?})", params);

        // initialize the set of known workspaces
        let mut folders: Vec<String> = Vec::new();
        if let Ok(mut workspace) = self.workspace.lock() {

            // initialize the workspace folders
            if let Some(workspace_folders) = params.workspace_folders {
                for folder in workspace_folders.iter() {
                    workspace.folders.push(folder.uri.clone());
                    if let Ok(path) = folder.uri.to_file_path() {
                        if let Some(path) = path.to_str() {
                            folders.push(path.to_string());
                        }
                    }
                }
            }

        }

        // initialize the indexer
        if let Ok(mut indexer) = self.indexer.lock() {
            indexer.start(folders);
        }

        // start a task to periodically flush logs
        // TODO: let log_push! notify the task so that logs can be flushed immediately,
        // instead of just polling
        let runtime = Handle::current();
        let client = self.client.clone();
        runtime.spawn(async move {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                crate::lsp::logger::flush(&client).await;
            }
        });

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "Amalthea R Kernel (ARK)".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                selection_range_provider: None,
                hover_provider: Some(HoverProviderCapability::from(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec!["$".to_string(), "@".to_string()]),
                    work_done_progress_options: Default::default(),
                    all_commit_characters: None,
                    ..Default::default()
                }),
                signature_help_provider: None,
                definition_provider: None,
                type_definition_provider: None,
                implementation_provider: None,
                references_provider: None,
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["dummy.do_something".to_string()],
                    work_done_progress_options: Default::default(),
                }),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, params: InitializedParams) {
        backend_trace!(self, "initialized({:?})", params);
    }

    async fn shutdown(&self) -> Result<()> {
        backend_trace!(self, "shutdown()");
        Ok(())
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        backend_trace!(self, "did_change_workspace_folders({:?})", params);
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        backend_trace!(self, "did_change_configuration({:?})", params);
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        backend_trace!(self, "did_change_watched_files({:?})", params);
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        backend_trace!(self, "execute_command({:?})", params);

        match self.client.apply_edit(WorkspaceEdit::default()).await {
            Ok(res) if res.applied => self.client.log_message(MessageType::INFO, "applied").await,
            Ok(_) => self.client.log_message(MessageType::INFO, "rejected").await,
            Err(err) => self.client.log_message(MessageType::ERROR, err).await,
        }

        Ok(None)
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        backend_trace!(self, "did_open({:?}", params);

        self.documents.insert(
            params.text_document.uri,
            Document::new(params.text_document.text.as_str()),
        );

    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        backend_trace!(self, "did_change({:?})", params);

        // get reference to document
        let uri = &params.text_document.uri;
        let mut doc = unwrap!(self.documents.get_mut(uri), {
            backend_trace!(self, "did_change(): unexpected document uri '{}'", uri);
            return;
        });

        // update the document
        for change in params.content_changes.iter() {
            doc.update(change);
        }

    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        backend_trace!(self, "did_save({:?}", params);
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        backend_trace!(self, "did_close({:?}", params);
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        backend_trace!(self, "completion({:?})", params);

        // get reference to document
        let uri = &params.text_document_position.text_document.uri;
        let mut document = unwrap!(self.documents.get_mut(uri), {
            backend_trace!(self, "completion(): No document associated with URI {}", uri);
            return Ok(None);
        });

        let mut completions : Vec<CompletionItem> = vec!();

        // add context-relevant completions
        append_document_completions(document.value_mut(), &params, &mut completions);

        return Ok(Some(CompletionResponse::Array(completions)));

    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        backend_trace!(self, "hover({:?})", params);
        Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::from_markdown(String::from(
                "Hello world!",
            ))),
            range: None,
        }))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        backend_trace!(self, "references({:?})", params);

        // TODO: Rather than searching files within the workspace on demand,
        // use an index of symbols built via a separate service thread.
        // Similar to the RStudio file monitor.

        // TODO: Figure out what kind of symbol the user is currently referencing.
        // Ideally, our 'Find References' implementation should be context-aware,
        // so that we can tell that these values are different:
        //
        //    foo <- function(value) { ... }
        //    data <- list(value = 42)
        //    data$value
        //
        // In general, refactoring 'names' is challenging; we have a bit more hope
        // with refactoring symbol names.
        if let Ok(workspace) = self.workspace.lock() {
            for folder in workspace.folders.iter() {
                if let Ok(path) = folder.to_file_path() {
                    let walker = WalkDir::new(path);
                    for entry in walker.into_iter().filter_entry(|entry| {
                        if let Some(name) = entry.file_name().to_str() {
                            match name {
                                ".git" | "node_modules" => {
                                    return false;
                                }

                                _ => { return true; }
                            }

                        }
                        return false;
                    }) {
                        if let Ok(entry) = entry {
                            log_push!("references(): {:?}", entry);
                            let path = entry.path();
                            if path.ends_with(".r") || path.ends_with(".R") {
                                log_push!("Found R script: {:?}", path);
                            }
                        }
                    }
                }

            }
        }

        let mut result = Vec::new();

        let range = Range::new(
            Position::new(0, 0),
            Position::new(5, 0),
        );

        result.push(Location {
            range: range,
            uri: params.text_document_position.text_document.uri,
        });

        Ok(Some(result))
    }
}

#[tokio::main]
pub async fn start_lsp(address: String) {
    #[cfg(feature = "runtime-agnostic")]
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    /*
    NOTE: The example LSP from tower-lsp uses a TcpListener, but we're using a
    TcpStream because -- according to LSP docs -- the client and server roles
    are reversed in terms of opening ports: the client listens, and the server a
    connection to it. The client and server can't BOTH listen on the port, so we
    let the client do it and connect to it here.

    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    */
    let stream = TcpStream::connect(address).await.unwrap();
    let (read, write) = tokio::io::split(stream);
    #[cfg(feature = "runtime-agnostic")]
    let (read, write) = (read.compat(), write.compat_write());

    let (service, socket) = LspService::new(|client| Backend {
        client: client,
        documents: DashMap::new(),
        indexer: Arc::new(Mutex::new(WorkspaceIndexer::new())),
        workspace: Arc::new(Mutex::new(Workspace::default())),
    });

    Server::new(read, write, socket).serve(service).await;
}
