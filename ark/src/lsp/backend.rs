/*
 * backend.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use dashmap::DashMap;
use ropey::Rope;
use serde_json::Value;
use tokio::net::TcpStream;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use tree_sitter::{Parser, TreeCursor, Node};

macro_rules! trace {

    ($self:expr, $($rest:expr),*) => {{
        let message = format!($($rest, )*);
        $self.client.log_message(MessageType::INFO, message).await
    }};

}

fn walk<F>(cursor: &mut TreeCursor, mut f: F)
where
    F: FnMut(Node)
{
    walk_impl(cursor, &mut f);
}

fn walk_impl<F>(cursor: &mut TreeCursor, f: &mut F)
where
    F: FnMut(Node),
{
    f(cursor.node());

    if cursor.goto_first_child() {

        walk_impl(cursor, f);
        while cursor.goto_next_sibling() {
            walk_impl(cursor, f);
        }

        cursor.goto_parent();

    }

}

#[derive(Debug)]
struct Backend {
    client: Client,
    documents: DashMap<String, Rope>,
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
        trace!(self, "initialized({:?})", params);
    }

    async fn shutdown(&self) -> Result<()> {
        trace!(self, "shutdown()");
        Ok(())
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        trace!(self, "did_change_workspace_folders({:?})", params);
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        trace!(self, "did_change_configuration({:?})", params);
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        trace!(self, "did_change_watched_files({:?})", params);
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        trace!(self, "execute_command({:?})", params);

        match self.client.apply_edit(WorkspaceEdit::default()).await {
            Ok(res) if res.applied => self.client.log_message(MessageType::INFO, "applied").await,
            Ok(_) => self.client.log_message(MessageType::INFO, "rejected").await,
            Err(err) => self.client.log_message(MessageType::ERROR, err).await,
        }

        Ok(None)
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        trace!(self, "did_open({:?}", params);

        // create document reference
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.documents.insert(uri.to_string(), Rope::from(text));

    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        trace!(self, "did_change({:?})", params);

        // get reference to document
        let uri = params.text_document.uri.to_string();
        let mut doc = self.documents.get_mut(&uri).unwrap_or_else(|| {
            panic!("unknown document '{}'", uri);
        });

        // update the document
        for change in params.content_changes.iter() {

            let range = match change.range {
                Some(r) => r,
                None => continue,
            };

            let lhs = doc.line_to_char(range.start.line as usize) + range.start.character as usize;
            let rhs = doc.line_to_char(range.end.line as usize) + range.end.character as usize;

            doc.remove(lhs..rhs);
            doc.insert(lhs, change.text.as_str());
            trace!(self, "document updated: {:?}", change);
            trace!(self, "document contents: {}", doc.to_string());

        }

    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        trace!(self, "did_save({:?}", params);
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        trace!(self, "did_close({:?}", params);
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {

        // find the document associated with this URI
        let uri = params.text_document_position.text_document.uri;
        let doc = match self.documents.get_mut(uri.as_str()) {
            Some(doc) => doc,
            None => {
                return Ok(None);
            }
        };

        // build AST from document
        // TODO: can we incrementally update AST as edits come in?
        // Or should we defer building the AST until completions are requested?
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_r::language()).expect("failed to create parser");

        let contents = doc.to_string();
        let ast = parser.parse(&contents, None).expect("failed to parse code");

        let mut completions : Vec<CompletionItem> = Vec::new();
        {
            let mut cursor = ast.walk();
            walk(&mut cursor, |node| {

                // check for assignments
                if node.kind() == "left_assignment" {
                    let lhs = node.child(0).unwrap();
                    if lhs.kind() == "identifier" {
                        let variable = lhs.utf8_text(contents.as_bytes());
                        if let Ok(variable) = variable {
                            let detail = format!("Defined on row {}", node.range().start_point.row + 1);
                            completions.push(CompletionItem::new_simple(variable.to_string(), detail));
                        }
                    }
                }

            });
        }

        return Ok(Some(CompletionResponse::Array(completions)));

    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        trace!(self, "hover({:?})", params);
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
