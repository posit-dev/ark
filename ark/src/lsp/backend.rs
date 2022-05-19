/*
 * backend.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::lsp::document::Document;
use crate::lsp::logger::LOGGER;
use crate::lsp::macros::{unwrap, backend_trace};
use dashmap::DashMap;
use serde_json::Value;
use tokio::net::TcpStream;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

#[derive(Debug)]
pub(crate) struct Backend {
    pub client: Client,
    pub documents: DashMap<Url, Document>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "Amalthea R Kernel (ARK)".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec!["$".to_string()]),
                    work_done_progress_options: Default::default(),
                    all_commit_characters: None,
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::from(true)),
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
            Document::new(params.text_document.text)
        );

    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        backend_trace!(self, "did_change({:?})", params);

        // get reference to document
        let uri = &params.text_document.uri;
        let mut doc = unwrap!(self.documents.get_mut(uri), {
            backend_trace!(self, "unexpected document uri '{}'", uri);
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
        let mut doc = unwrap!(self.documents.get_mut(uri), {
            backend_trace!(self, "unexpected document uri '{}'", uri);
            return Ok(None);
        });

        let mut completions : Vec<CompletionItem> = vec!();

        // add context-relevant completions
        doc.append_completions(&params, &mut completions);

        unsafe { LOGGER.flush(self).await; }

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
        documents: DashMap::new()
    });

    Server::new(read, write, socket).serve(service).await;
}
