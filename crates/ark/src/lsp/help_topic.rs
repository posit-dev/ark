//
// help_topic.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use serde::Deserialize;
use serde::Serialize;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;

use crate::backend_trace;
use crate::lsp::backend::Backend;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::position::PositionExt;

pub static POSITRON_HELP_TOPIC_REQUEST: &'static str = "positron/textDocument/helpTopic";

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpTopicParams {
    /// The document to provide a help topic for.
    pub text_document: VersionedTextDocumentIdentifier,
    /// The location of the cursor.
    pub position: Position,
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpTopicResponse {
    /// The help topic appropriate for the cursor position.
    pub topic: String,
}

impl Backend {
    pub async fn help_topic(
        &self,
        params: HelpTopicParams,
    ) -> tower_lsp::jsonrpc::Result<Option<HelpTopicResponse>> {
        backend_trace!(self, "help_topic({:?})", params);

        let uri = &params.text_document.uri;
        let Some(document) = self.documents.get_mut(uri) else {
            backend_trace!(self, "help_topic(): No document associated with URI {uri}");
            return Ok(None);
        };

        let root = document.ast.root_node();
        let position = params.position;
        let point = position.as_point();

        let mut cursor = root.walk();
        let node = cursor.find_leaf(point);

        // Check to see whether it's a function node (currently that's what we
        // support)
        if node.kind() != "function" {
            backend_trace!(self, "help_topic(): No function node at position {point}");
            return Ok(None);
        }

        // Get the text of the node
        let source = document.contents.to_string();
        let text = node.utf8_text(source.as_bytes()).unwrap();

        let response = HelpTopicResponse {
            topic: String::from(text),
        };

        Ok(Some(response))
    }
}
