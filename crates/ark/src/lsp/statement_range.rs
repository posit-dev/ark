//
// statement_range.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::bail;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use stdext::unwrap;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;
use tree_sitter::Node;

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
    ) -> tower_lsp::jsonrpc::Result<Option<StatementRangeResponse>> {
        backend_trace!(self, "statement_range({:?})", params);

        let uri = &params.text_document.uri;
        let document = unwrap!(self.documents.get_mut(uri), None => {
            backend_trace!(self, "statement_range(): No document associated with URI {uri}");
            return Ok(None);
        });

        let root = document.ast.root_node();

        let position = params.position;
        let point = position.as_point();
        let row = point.row;

        let node = unwrap!(find_statement_range_node(root, row), None => {
            return Ok(None)
        });

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

fn find_statement_range_node(root: Node, row: usize) -> Option<Node> {
    let mut cursor = root.walk();

    let children = root.children(&mut cursor);

    let mut out = None;

    for child in children {
        if row > child.end_position().row {
            // Find the first child who's end position row extends past or is
            // equal to the user selected `row`
            continue;
        }
        if child.kind() == "comment" {
            // Skip comments
            continue;
        }

        // If there was a node associated with the point, recurse into it
        // to figure out exactly which range to execute
        match recurse(child, row) {
            Ok(node) => {
                out = node;
            },
            Err(error) => {
                log::error!("Failed to find statement range node due to: {error}.");
            },
        }

        break;
    }

    out
}

fn recurse(node: Node, row: usize) -> Result<Option<Node>> {
    // General row-based heuristics that apply to all node types
    if row <= node.start_position().row {
        // On or before node row, execute whole node
        return Ok(Some(node));
    }
    if row == node.end_position().row {
        // On closing node row, execute whole node
        // Note: This applies even for things like functions, which typically
        // end with `}` but don't have to. We always execute the whole statement
        // here to avoid sending just a block node without its leading
        // `function` node.
        return Ok(Some(node));
    }

    match node.kind() {
        "function" => recurse_function(node, row),
        "for" | "while" | "repeat" => recurse_loop(node, row),
        "if" => recurse_if(node, row),
        "{" => recurse_block(node, row),
        "[" | "[[" => recurse_subset(node, row),
        "call" => recurse_call(node, row),
        _ => recurse_default(node, row),
    }
}

fn recurse_function(node: Node, row: usize) -> Result<Option<Node>> {
    let parameters = unwrap!(node.child_by_field_name("parameters"), None => {
        bail!("Missing `parameters` field in a `function` node");
    });

    if parameters.start_position().row <= row && parameters.end_position().row >= row {
        // If we are inside the parameters list, execute entire function
        // (parameter lists often span multiple lines for long signatures)
        return Ok(Some(node));
    }

    if let Some(body) = node.child_by_field_name("body") {
        // If we are somewhere inside the body, then we only want to execute
        // the particular expression the cursor is over
        if body.start_position().row <= row && body.end_position().row >= row {
            return recurse(body, row);
        }
    }

    // If we had a detectable `function` node and nothing else matched,
    // just run the whole function
    Ok(Some(node))
}

fn recurse_loop(node: Node, row: usize) -> Result<Option<Node>> {
    let body = unwrap!(node.child_by_field_name("body"), None => {
        // Rare, but no body is possible, just send whole loop node anyways
        return Ok(Some(node));
    });

    // Are we placed on a statement inside braces? If so, run just that
    // statement.
    let candidate = contains_row_at_different_start_position(body, row);
    if candidate.is_some() {
        return Ok(candidate);
    }

    // Otherwise run whole loop node
    Ok(Some(node))
}

fn recurse_if(node: Node, row: usize) -> Result<Option<Node>> {
    let consequence = unwrap!(node.child_by_field_name("consequence"), None => {
        bail!("Missing `consequence` child in an `if` node.");
    });
    if row == consequence.start_position().row {
        // On start row of the `if` branch, likely a `{` so just execute whole statement
        return Ok(Some(node));
    }
    if row == consequence.end_position().row {
        // On end row of the `if` branch, likely a `}` so just execute whole statement.
        return Ok(Some(node));
    }

    let candidate = contains_row_at_different_start_position(consequence, row);
    if candidate.is_some() {
        // The `if` branch contains the user's `row` and the `row` is on a
        // standalone line that should be executed on its own
        return Ok(candidate);
    }

    let alternative = unwrap!(node.child_by_field_name("alternative"), None => {
        // No `else` and nothing above matched, execute whole if statement
        return Ok(Some(node))
    });
    if row == alternative.start_position().row {
        // On start row of the `else` branch, likely a `{` so just execute whole statement
        return Ok(Some(node));
    }
    if row == alternative.end_position().row {
        // On end row of the `else` branch, likely a `}` so just execute whole statement.
        return Ok(Some(node));
    }

    let candidate = contains_row_at_different_start_position(alternative, row);
    if candidate.is_some() {
        // The `else` branch contains the user's `row` and the `row` is on a
        // standalone line that should be executed on its own
        return Ok(candidate);
    }

    // If we get here we may be in the if statement's `condition` node or on
    // a random newline that the user may have inserted inside the `if` node
    Ok(Some(node))
}

fn recurse_call(node: Node, row: usize) -> Result<Option<Node>> {
    let arguments = unwrap!(node.child_by_field_name("arguments"), None => {
        bail!("Missing `arguments` field in a call node");
    });
    if row == arguments.start_position().row {
        // On start row containing `(`, execute whole call
        return Ok(Some(node));
    }
    if row == arguments.end_position().row {
        // On ending row containing `)`, execute whole call
        return Ok(Some(node));
    }

    // In general if the user executes a statement while on a function argument,
    // then we execute the entire function call. However, calls like
    // test_that("foo", {
    //   1 + 1
    // })
    // purposefully span multiple lines and you want to be able to execute
    // `1 + 1` interactively (similar with `withr::local_*()` calls).
    // To detect this we use a heuristic that if the argument `value` node has a
    // different start row than the row you'd get by recursing into that `value`
    // node, then we prefer the row from the recursion, otherwise we execute the
    // entire function call.
    let mut cursor = arguments.walk();
    let children = arguments.children_by_field_name("argument", &mut cursor);

    for child in children {
        let value = unwrap!(child.child_by_field_name("value"), None => {
            // Rare, but can have no value node
            continue;
        });

        let candidate = contains_row_at_different_start_position(value, row);
        if candidate.is_some() {
            return Ok(candidate);
        }
    }

    Ok(Some(node))
}

fn recurse_block(node: Node, row: usize) -> Result<Option<Node>> {
    // Recurse into body statements if you are somewhere inside the block
    let mut cursor = node.walk();

    for child in node.children_by_field_name("body", &mut cursor) {
        if row > child.end_position().row {
            // Find the first child who's end position row extends past or is
            // equal to the user selected `row`
            continue;
        }
        if child.kind() == "comment" {
            // Skip comments
            continue;
        }

        // If there was a node associated with the point, recurse into it
        // to figure out exactly which range to execute
        return recurse(child, row);
    }

    Ok(None)
}

fn recurse_subset(node: Node, _row: usize) -> Result<Option<Node>> {
    // Assume that if you've created a multi-line subset call with `[` (like
    // with data.table) then you probably want to send the whole statement
    Ok(Some(node))
}

fn recurse_default(node: Node, row: usize) -> Result<Option<Node>> {
    // For default nodes, we need to check the children to see if there are any
    // `{` blocks that the cursor could have been contained in
    let mut cursor = node.walk();
    let children = node.children(&mut cursor);

    for child in children {
        let candidate = contains_row_at_different_start_position(child, row);

        if candidate.is_some() {
            return Ok(candidate);
        }
    }

    Ok(Some(node))
}

/// Checks if we can recurse into `node` to match the `row` to a child node
/// that is on a different starting row than `node` is on (likely implying the
/// user has braces somewhere in the expression and has placed the cursor on a
/// line inside those braces).
///
/// Returns `None` if no candidate node is detected, otherwise returns
/// `Some(candidate)` containing the candidate node
fn contains_row_at_different_start_position(node: Node, row: usize) -> Option<Node> {
    if !(node.start_position().row <= row && row <= node.end_position().row) {
        // Doesn't contain the row the user is on
        return None;
    }
    if node.start_position().row == node.end_position().row {
        // Doesn't span multiple lines, impossible to have a child on a different line
        return None;
    }

    // Ok, the node spans multiple lines and contains `row`, see if the
    // candidate node starts on a different line than the original node
    let candidate = match recurse(node, row) {
        // We either found a candidate node or returned `None`
        Ok(node) => unwrap!(node, None => return None),
        // Ignoring possible errors
        Err(_) => return None,
    };

    if node.start_position().row == candidate.start_position().row {
        // Same start row
        None
    } else {
        // Differing start row, so `candidate` takes priority
        Some(candidate)
    }
}

#[test]
fn test_statement_range() {
    use tree_sitter::Parser;
    use tree_sitter::Point;
    use tree_sitter::Range;

    let find_statement_range_node_test = |contents: &str, row: usize| -> Range {
        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_r::language())
            .expect("Failed to create parser");
        let ast = parser.parse(contents, None).unwrap();
        let root = ast.root_node();
        find_statement_range_node(root, row).unwrap().range()
    };

    let row = 0;
    let contents = "1 + 1";
    assert_eq!(
        find_statement_range_node_test(contents, row)
            .start_point
            .row,
        0
    );

    // Finds next row
    let row = 0;
    let contents = "
1 + 1
";
    assert_eq!(
        find_statement_range_node_test(contents, row)
            .start_point
            .row,
        1
    );

    // Finds next row
    let row = 0;
    let contents = "



1 + 1
";
    assert_eq!(
        find_statement_range_node_test(contents, row)
            .start_point
            .row,
        4
    );

    // Executes all braces
    let row = 0;
    let contents = "
{
  1 + 1
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 3, column: 1 });

    // Inside braces, runs the statement the cursor is on
    let row = 2;
    let contents = "
{
  1 + 1
  2 + 2
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 2 });
    assert_eq!(node.end_point, Point { row: 2, column: 7 });

    // Executes entire function
    let row = 0;
    let contents = "
function() {
  1 + 1
  2 + 2
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 4, column: 1 });

    // Executes individual lines of a function if user puts cursor there
    let row = 3;
    let contents = "
function() {
  1 + 1
  2 + 2
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 3, column: 2 });
    assert_eq!(node.end_point, Point { row: 3, column: 7 });

    // Executes entire function if on multiline argument signature
    let row = 2;
    let contents = "
function(a,
         b,
         c) {
  1 + 1
  2 + 2
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 6, column: 1 });

    // Executes entire loop if on first or last row
    let contents = "
for(i in 1:5) {
  print(i)
  1 + 1
}
";
    let row = 1;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 4, column: 1 });

    let row = 4;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 4, column: 1 });

    // But if inside the braces, runs the line the user was on
    let row = 2;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 2 });
    assert_eq!(node.end_point, Point { row: 2, column: 10 });

    // Function within function executes whole subfunction
    let row = 3;
    let contents = "
function() {
  1 + 1

  function(a) {
    2 + 2
  }
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 4, column: 2 });
    assert_eq!(node.end_point, Point { row: 6, column: 3 });

    // Function with weird signature setup works as expected
    let contents = "
function
(a,
    b
)
{
    1 + 1
}
";
    let row = 1;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 7, column: 1 });

    let row = 2;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 7, column: 1 });

    let row = 3;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 7, column: 1 });

    let row = 7;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 7, column: 1 });

    // Function with newlines runs whole function
    let row = 2;
    let contents = "
function()


{
    1 + 1
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 6, column: 1 });

    // `if` statements run whole statement where appropriate
    let contents = "
if (a > b) {
    1 + 1
} else if (b > c) {
    2 + 2
    3 + 3
} else {
    4 + 4
}
";
    let row = 1;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 8, column: 1 });

    let row = 3;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 8, column: 1 });

    let row = 6;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 8, column: 1 });

    let row = 8;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 8, column: 1 });

    // Inside braces, runs individual statement
    let row = 5;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 5, column: 4 });
    assert_eq!(node.end_point, Point { row: 5, column: 9 });

    let row = 7;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 7, column: 4 });
    assert_eq!(node.end_point, Point { row: 7, column: 9 });

    // Subsetting runs whole expression
    let row = 3;
    let contents = "
dt[
  a > b,
  by = 4,
  foo
]
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 5, column: 1 });

    // Blocks within calls run one line at a time (testthat, withr, quote())
    let row = 2;
    let contents = "
test_that('stuff', {
    x <- 1
    y <- 2
    expect_equal(x, y)
})
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 4 });
    assert_eq!(node.end_point, Point { row: 2, column: 10 });

    // But can run entire expression
    let row = 1;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 5, column: 2 });

    let row = 5;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 5, column: 2 });

    // Comments are skipped from root level
    let row = 0;
    let contents = "
# hi there

# another one

1 + 1
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 5, column: 0 });
    assert_eq!(node.end_point, Point { row: 5, column: 5 });

    // Comments are skipped in blocks
    let row = 2;
    let contents = "
{
    # hi there

    # another one

    1 + 1
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 6, column: 4 });
    assert_eq!(node.end_point, Point { row: 6, column: 9 });

    // Unmatched opening braces send the full partial statement
    let row = 0;
    let contents = "
{
    1 + 1

";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 4, column: 0 });

    // Binary op with braces respects that you can put the cursor inside the braces
    let row = 2;
    let contents = "
1 + {
    2 + 2
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 4 });
    assert_eq!(node.end_point, Point { row: 2, column: 9 });

    // Will return `None` when there is no top level statement
    let row = 2;
    let contents = "
1 + 1


";
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_r::language())
        .expect("Failed to create parser");
    let ast = parser.parse(contents, None).unwrap();
    let root = ast.root_node();
    assert_eq!(find_statement_range_node(root, row), None);

    // Will return `None` when there is no block level statement
    let row = 3;
    let contents = "
{
    1 + 1


}
";
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_r::language())
        .expect("Failed to create parser");
    let ast = parser.parse(contents, None).unwrap();
    let root = ast.root_node();
    assert_eq!(find_statement_range_node(root, row), None);
}
