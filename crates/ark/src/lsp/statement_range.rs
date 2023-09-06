//
// statement_range.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use serde::Deserialize;
use serde::Serialize;
use stdext::unwrap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;

use crate::backend_trace;
use crate::lsp::backend::Backend;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::position::PositionExt;

pub static POSITRON_STATEMENT_RANGE_REQUEST: &'static str = "positron/textDocument/statementRange";

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRangeParams {
    /// The document to provide a statement range for.
    pub text_document: VersionedTextDocumentIdentifier,
    /// The location of the cursor.
    pub position: Position,
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRangeResponse {
    /// The document range the statement covers.
    pub range: Range,
}

impl Backend {
    pub async fn statement_range(
        &self,
        params: StatementRangeParams,
    ) -> Result<Option<StatementRangeResponse>> {
        backend_trace!(self, "statement_range({:?})", params);

        let uri = &params.text_document.uri;
        let document = unwrap!(self.documents.get_mut(uri), None => {
            backend_trace!(self, "statement_range(): No document associated with URI {uri}");
            return Ok(None);
        });

        let position = params.position;
        let point = position.as_point();

        let root = document.ast.root_node();
        let mut cursor = root.walk();

        if !cursor.goto_first_child_for_point_patched(point) {
            // TODO: Uncommenting this causes a compile error???
            // backend_trace!(self, "statement_range(): No child associated with point.");
            return Ok(None);
        }

        let node = cursor.node();

        // Tree-sitter `Point`s
        let start_point = node.start_position();
        let end_point = node.end_position();

        // To LSP `Position`s
        let start_position = start_point.as_position();
        let end_position = end_point.as_position();

        let range = Range {
            start: start_position,
            end: end_position,
        };

        let response = StatementRangeResponse { range };

        Ok(Some(response))
    }
}
