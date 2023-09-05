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
use tree_sitter::Point;
use tree_sitter::TreeCursor;

use crate::backend_trace;
use crate::lsp::backend::Backend;
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

        if !Backend::goto_first_child_for_point(&mut cursor, point) {
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

    /// Move this cursor to the first child of its current node that extends
    /// beyond or touches the given point. Returns `true` if a child node was found,
    /// otherwise returns `false`.
    ///
    /// TODO: In theory we should be using `cursor.goto_first_child_for_point()`,
    /// but it is reported to be broken, and indeed does not work right if I
    /// substitute it in.
    /// https://github.com/tree-sitter/tree-sitter/issues/2012
    ///
    /// This simple reimplementation is based on this Emacs hot patch
    /// https://git.savannah.gnu.org/cgit/emacs.git/commit/?h=emacs-29&id=7c61a304104fe3a35c47d412150d29b93a697c5e
    fn goto_first_child_for_point(cursor: &mut TreeCursor, point: Point) -> bool {
        if !cursor.goto_first_child() {
            return false;
        }

        let mut node = cursor.node();

        // The emacs patch used `<=` in the while condition, but we want the
        // following to execute all of `fn()` if the cursor is placed at the `|`
        // fn <- function() {
        // }|
        while node.end_position() < point {
            if cursor.goto_next_sibling() {
                node = cursor.node();
            } else {
                // Reached the end and still can't find a valid sibling
                return false;
            }
        }

        return true;
    }
}
