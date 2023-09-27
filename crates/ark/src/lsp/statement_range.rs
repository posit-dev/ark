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
    // General row-based heuristic that apply to all node types.
    // If we are on or before the node row, execute whole node.
    // End position behavior is node kind dependent.
    if row <= node.start_position().row {
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

    let Some(body) = node.child_by_field_name("body") else {
        // No `body`, execute entire function
        return Ok(Some(node));
    };

    if row < body.start_position().row || row > body.end_position().row {
        // We are outside the `body`, so no need to continue recursing
        // (possibly on a newline a user inserted between the parameters and body)
        return Ok(Some(node));
    }

    if body.kind() == "{" && (row == body.start_position().row || row == body.end_position().row) {
        // For the most common `{` bodies, if we are on the `{` or the `}` rows, then we execute the
        // entire function. This avoids sending a `{` block without its leading `function` node if
        // `{` happens to be on a different line or if the user is on the `}` line.
        return Ok(Some(node));
    }

    // If we are somewhere inside the body, then we only want to execute
    // the particular expression the cursor is over
    recurse(body, row)
}

fn recurse_loop(node: Node, row: usize) -> Result<Option<Node>> {
    let Some(body) = node.child_by_field_name("body") else {
        // Rare, but no body is possible, just send whole loop node anyways
        return Ok(Some(node));
    };

    if !(row >= body.start_position().row && row <= body.end_position().row) {
        // We aren't in the `body` at all. Might be on newlines inserted by the user
        // between the `for/while/repeat` line and the `body`, or in a `condition` node
        // that spans multiple lines. In this case, run the whole node.
        return Ok(Some(node));
    }

    if body.kind() == "{" && (row == body.start_position().row || row == body.end_position().row) {
        // For the most common `{` bodies, if we are on the `{` or the `}` rows, then we execute the
        // entire loop. This avoids sending a `{` block without its leading loop node if
        // `{` happens to be on a different line or if the user is on the `}` line.
        return Ok(Some(node));
    }

    // If we are somewhere inside the body, then we only want to execute
    // the particular expression the cursor is over
    recurse(body, row)
}

fn recurse_if(node: Node, row: usize) -> Result<Option<Node>> {
    let Some(consequence) = node.child_by_field_name("consequence") else {
        bail!("Missing `consequence` child in an `if` node.");
    };
    if row >= consequence.start_position().row && row <= consequence.end_position().row {
        // We are somewhere inside the `consequence`

        if consequence.kind() == "{" &&
            (row == consequence.start_position().row || row == consequence.end_position().row)
        {
            // On `{` or `}` row of a `{` node, execute entire if statement
            return Ok(Some(node));
        }

        // If the `consequence` contains the user row and we aren't on a `{` or `}` row,
        // then we only want to run the expression the cursor is over
        return recurse(consequence, row);
    }

    let Some(alternative) = node.child_by_field_name("alternative") else {
        // No `else` and nothing above matched, execute whole if statement
        return Ok(Some(node));
    };
    if row >= alternative.start_position().row && row <= alternative.end_position().row {
        // We are somewhere inside the `alternative`, possibly in an `else if`

        if alternative.kind() == "{" &&
            (row == alternative.start_position().row || row == alternative.end_position().row)
        {
            // On `{` or `}` row of a `{` node, execute entire if statement
            return Ok(Some(node));
        }

        if alternative.kind() == "if" {
            // We are inside an `else if {` case. See if recursing over this `if` node
            // results in a new start position row.
            let Some(candidate) = recurse_if(alternative, row)? else {
                // No result from recursing over `if` node, send entire original `if` statement
                return Ok(Some(node));
            };

            if alternative.start_position().row == candidate.start_position().row {
                // Use original `if` node since it looks like we are on an `else if` or `{` or `}` line
                return Ok(Some(node));
            } else {
                // Otherwise assume `candidate` is a standalone row
                return Ok(Some(candidate));
            }
        }

        // If the `alternative` contains the user row and we aren't on a `{` or `}` row or in another `if` block,
        // then we only want to run the expression the cursor is over
        return recurse(alternative, row);
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
        let Some(value) = child.child_by_field_name("value") else {
            // Rare, but can have no value node
            continue;
        };

        let candidate = contains_row_at_different_start_position(value, row);
        if candidate.is_some() {
            return Ok(candidate);
        }
    }

    Ok(Some(node))
}

fn recurse_block(node: Node, row: usize) -> Result<Option<Node>> {
    if row == node.end_position().row {
        // `recurse()` handled the start position, but if we are on the
        // `}` row, then we also execute the entire block
        return Ok(Some(node));
    }

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

    // We are likely on some blank line after the last `body` child,
    // but before the closing `}`. In this case we don't send anything.
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
    let contents = "
function() {
  1 + 1
  2 + 2
}
";
    let row = 0;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 4, column: 1 });

    let row = 4;
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

    // Executes just the expression if on a 1 line function
    let row = 2;
    let contents = "
function()
  1 + 1
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 2 });
    assert_eq!(node.end_point, Point { row: 2, column: 7 });

    // Executes just the expression if on a 1 line function in an assignment
    let row = 2;
    let contents = "
fn <- function()
  1 + 1
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 2 });
    assert_eq!(node.end_point, Point { row: 2, column: 7 });

    // Executes entire function if on a `{` that is on its own line
    let row = 2;
    let contents = "
fn <- function()
{
    1 + 1
}
";
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 4, column: 1 });

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

    // Executes just expression if on a 1 line loop with no braces
    let contents = "
for(i in 1:5)
  print(1)
";
    let row = 2;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 2 });
    assert_eq!(node.end_point, Point { row: 2, column: 10 });

    // Executes entire loop if on a `{` that is on its own line
    let contents = "
for(i in 1:5)
{
    print(1)
}
";
    let row = 2;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 4, column: 1 });

    // Executes entire loop if on a `condition` that is on its own line
    let contents = "
for
(i in 1:5)
{
    1 + 1
}
";
    let row = 2;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 5, column: 1 });

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

    // `if` statements without braces can run individual expressions
    let contents = "
if (a > b)
  1 + 1
";

    let row = 1;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    // TODO: This is a tree-sitter bug! It should only go to row: 2, column: 7.
    assert_eq!(node.end_point, Point { row: 3, column: 0 });

    let row = 2;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 2 });
    assert_eq!(node.end_point, Point { row: 2, column: 7 });

    // `if`-else statements without braces can run individual expressions
    let contents = "
if (a > b)
  1 + 1
else if (b > c)
  2 + 2
else
  4 + 4
";
    let row = 1;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 6, column: 7 });

    let row = 2;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 2, column: 2 });
    assert_eq!(node.end_point, Point { row: 2, column: 7 });

    let row = 3;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 6, column: 7 });

    let row = 4;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 4, column: 2 });
    assert_eq!(node.end_point, Point { row: 4, column: 7 });

    let row = 5;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    assert_eq!(node.end_point, Point { row: 6, column: 7 });

    let row = 6;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 6, column: 2 });
    assert_eq!(node.end_point, Point { row: 6, column: 7 });

    // TODO: This test should fail once we fix the tree-sitter bug.
    // `if` statements without an `else` don't consume newlines
    let contents = "
if (a > b) {
    1 + 1
}


";

    let row = 1;
    let node = find_statement_range_node_test(contents, row);
    assert_eq!(node.start_point, Point { row: 1, column: 0 });
    // TODO: It should only go to row: 3, column: 1.
    assert_eq!(node.end_point, Point { row: 6, column: 0 });

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
