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
        let Some(document) = self.documents.get_mut(uri) else {
            backend_trace!(
                self,
                "statement_range(): No document associated with URI {uri}"
            );
            return Ok(None);
        };

        let root = document.ast.root_node();

        let position = params.position;
        let point = position.as_point();
        let row = point.row;

        let Some(node) = find_statement_range_node(root, row) else {
            return Ok(None);
        };

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
    let Some(parameters) = node.child_by_field_name("parameters") else {
        bail!("Missing `parameters` field in a `function` node");
    };

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
    let Some(arguments) = node.child_by_field_name("arguments") else {
        bail!("Missing `arguments` field in a call node");
    };
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

#[rustfmt::skip]
#[test]
fn test_statement_range() {
    use tree_sitter::Parser;
    use tree_sitter::Point;

    // Intended to ease statement range testing. Supply `x` as a string containing
    // the expression to test along with:
    // - `@` marking the cursor position
    // - `<<` marking the expected selection start position
    // - `>>` marking the expected selection end position
    // These characters will be replaced with the empty string before being parsed
    // by tree-sitter. It is generally best to left align the string against the
    // far left margin to avoid unexpected whitespace and mimic real life.
    fn statement_range_test(x: &str) {
        let lines = x.split("\n").collect::<Vec<&str>>();

        let mut cursor: Option<Point> = None;
        let mut sel_start: Option<Point> = None;
        let mut sel_end: Option<Point> = None;

        let mark_start = b'<';
        let mark_cursor = b'@';
        let mark_end = b'>';

        let mut in_start = false;
        let mut in_end = false;

        for (line_row, line) in lines.into_iter().enumerate() {
            for (char_column, char) in line.as_bytes().into_iter().enumerate() {
                if in_start {
                    // We are in a `<`. Whatever happens next, we will exit the "in start" state.
                    in_start = false;

                    // Found a `<<`
                    if char == &mark_start {
                        if !sel_end.is_none() {
                            panic!("`<<` must be used before `>>`.");
                        }
                        if !sel_start.is_none() {
                            panic!("`<<` must only be used once.");
                        }

                        // `adjustment = 1` is for the 2 byte wide `<<`
                        let adjustment = 1;

                        let adjustment2 = match cursor {
                            Some(cursor) => {
                                (cursor.row == line_row && cursor.column < char_column) as usize
                            },
                            None => 0,
                        };

                        sel_start = Some(Point {
                            row: line_row,
                            column: char_column - adjustment - adjustment2,
                        });

                        continue;
                    }
                }

                if in_end {
                    // We are in a `>`. Whatever happens next, we will exit the "in end" state.
                    in_end = false;

                    // Found a `>>`
                    if char == &mark_end {
                        if sel_start.is_none() {
                            panic!("`<<` must be used before `>>`.");
                        }
                        if !sel_end.is_none() {
                            panic!("`>>` must only be used once.");
                        }

                        // `adjustment = 1` is for the 2 byte wide `>>`
                        let adjustment = 1;

                        let adjustment2 = match sel_start {
                            Some(sel_start) => {
                                (sel_start.row == line_row && sel_start.column < char_column)
                                    as usize
                            },
                            None => 0,
                        };
                        let adjustment2 = adjustment2 * 2;

                        let adjustment3 = match cursor {
                            Some(cursor) => {
                                (cursor.row == line_row && cursor.column < char_column) as usize
                            },
                            None => 0,
                        };

                        sel_end = Some(Point {
                            row: line_row,
                            column: char_column - adjustment - adjustment2 - adjustment3,
                        });

                        continue;
                    }
                }

                if char == &mark_start {
                    in_start = true;
                    continue;
                }

                if char == &mark_end {
                    in_end = true;
                    continue;
                }

                if char == &mark_cursor {
                    if !cursor.is_none() {
                        panic!("`@` must only be used once.");
                    }

                    let adjustment = match sel_start {
                        Some(sel_start) => {
                            (sel_start.row == line_row && sel_start.column < char_column) as usize
                        },
                        None => 0,
                    };
                    let adjustment = adjustment * 2;

                    let adjustment2 = match sel_end {
                        Some(sel_end) => {
                            (sel_end.row == line_row && sel_end.column < char_column) as usize
                        },
                        None => 0,
                    };
                    let adjustment2 = adjustment2 * 2;

                    cursor = Some(Point {
                        row: line_row,
                        column: char_column - adjustment - adjustment2,
                    });

                    continue;
                }
            }
        }

        if cursor.is_none() || sel_start.is_none() || sel_end.is_none() {
            panic!("`<<`, `@`, and `>>` must all be used.");
        }

        // Replace mark characters with empty string.
        // We adjusted column positions for this already.
        // (i.e. create the R parsable string assuming those characters weren't there)
        let x = x.replace("<<", "");
        let x = x.replace("@", "");
        let x = x.replace(">>", "");

        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_r::language())
            .expect("Failed to create parser");

        let ast = parser.parse(x, None).unwrap();

        let root = ast.root_node();

        let node = find_statement_range_node(root, cursor.unwrap().row).unwrap();

        assert_eq!(node.start_position(), sel_start.unwrap());
        assert_eq!(node.end_position(), sel_end.unwrap());
    }

    // Simple test
    statement_range_test("<<1@+ 1>>");

    // Finds next row
    statement_range_test(
"
@
<<1 + 1>>
",
    );

    // Finds next row with many spaces
    statement_range_test(
"
@



<<1 + 1>>
",
    );

    // Executes all braces
    statement_range_test(
"
@
<<{
    1 + 1
}>>
",
    );

    // Inside braces, runs the statement the cursor is on
    statement_range_test(
"
{
    @<<1 + 1>>
    2 + 2
}
",
    );

    // Executes entire function
    statement_range_test(
"
@
<<function() {
    1 + 1
    2 + 2
}>>
",
    );
    statement_range_test(
"
<<function() {
    1 + 1
    2 + 2
}>>@
",
    );

    // Executes individual lines of a function if user puts cursor there
    statement_range_test(
"
function() {
    1 + 1
    <<2 + @2>>
}
",
    );

    // Executes entire function if on multiline argument signature
    statement_range_test(
"
<<function(a,
            b,@
            c) {
    1 + 1
    2 + 2
}>>
",
    );

    // Executes just the expression if on a 1 line function
    statement_range_test(
"
function()
    @<<1 + 1>>
",
    );

    // Executes just the expression if on a 1 line function in an assignment
    statement_range_test(
"
fn <- function()
    @<<1 + 1>>
",
    );

    // Executes entire function if on a `{` that is on its own line
    statement_range_test(
"
<<fn <- function()
{@
    1 + 1
}>>
",
    );

    // Executes entire loop if on first or last row
    statement_range_test(
"
<<for(i@ in 1:5) {
    print(i)
    1 + 1
}>>
",
    );
    statement_range_test(
"
<<for(i in 1:5) {
    print(i)
    1 + 1
}@>>
",
    );

    // But if inside the braces, runs the line the user was on
    statement_range_test(
"
for(i in 1:5) {
    <<print@(i)>>
    1 + 1
}
",
    );

    // Executes just expression if on a 1 line loop with no braces
    statement_range_test(
"
for(i in 1:5)
    <<print(1)@>>
",
    );

    // Executes entire loop if on a `{` that is on its own line
    statement_range_test(
"
<<for(i in 1:5)
{@
    print(1)
}>>
",
    );

    // Executes entire loop if on a `condition` that is on its own line
    statement_range_test(
"
<<for
(i in @1:5)
{
    1 + 1
}>>
",
    );

    // Function within function executes whole subfunction
    statement_range_test(
"
function() {
    1 + 1
    @
    <<function(a) {
    2 + 2
    }>>
}
",
    );

    // Function with weird signature setup works as expected
    statement_range_test(
"
<<function@
(a,
    b
)
{
    1 + 1
}>>
",
    );
    statement_range_test(
"
<<function
(a@,
    b
)
{
    1 + 1
}>>
",
    );
    statement_range_test(
"
<<function
(a,
    b@
)
{
    1 + 1
}>>
",
    );
    statement_range_test(
"
<<function
(a,
    b
)
{
    1 + 1
}@>>
",
    );

    // Function with newlines runs whole function
    statement_range_test(
"
<<function()
@

{
    1 + 1
}>>
",
    );

    // `if` statements run whole statement where appropriate
    statement_range_test(
"
<<if @(a > b) {
    1 + 1
} else if (b > c) {
    2 + 2
    3 + 3
} else {
    4 + 4
}>>
",
    );
    statement_range_test(
"
<<if (a > b) {
    1 + 1
} else if @(b > c) {
    2 + 2
    3 + 3
} else {
    4 + 4
}>>
",
    );
    statement_range_test(
"
<<if (a > b) {
    1 + 1
} else if (b > c) {
    2 + 2
    3 + 3
} else {@
    4 + 4
}>>
",
    );
    statement_range_test(
"
<<if (a > b) {
    1 + 1
} else if (b > c) {
    2 + 2
    3 + 3
} else {
    4 + 4
}@>>
",
    );

    // Inside braces, runs individual statement
    statement_range_test(
"
if (a > b) {
    1 + 1
} else if (b > c) {
    2 + 2
    <<3 + @3>>
} else {
    4 + 4
}
",
    );
    statement_range_test(
"
if (a > b) {
    1 + 1
} else if (b > c) {
    2 + 2
    3 + 3
} else {
    <<@4 + 4>>
}
",
    );

    // `if` statements without braces can run individual expressions
    // TODO: This is a tree-sitter bug! It should only go to row: 2, column: 7.
    statement_range_test(
"
<<if (@a > b)
    1 + 1
>>",
    );
    statement_range_test(
"
if (a > b)
  <<1 + 1@>>
",
    );

    // `if`-else statements without braces can run individual expressions
    statement_range_test(
"
<<if @(a > b)
  1 + 1
else if (b > c)
  2 + 2
else
  4 + 4>>
",
    );
    statement_range_test(
"
if (a > b)
  <<@1 + 1>>
else if (b > c)
  2 + 2
else
  4 + 4
",
    );
    statement_range_test(
"
<<if (a > b)
  1 + 1
else if @(b > c)
  2 + 2
else
  4 + 4>>
",
    );
    statement_range_test(
"
if (a > b)
  1 + 1
else if (b > c)
  <<2 + @2>>
else
  4 + 4
",
    );
    statement_range_test(
"
<<if (a > b)
  1 + 1
else if (b > c)
  2 + 2
else@
  4 + 4>>
",
    );
    statement_range_test(
"
if (a > b)
  1 + 1
else if (b > c)
  2 + 2
else
  <<4 @+ 4>>
",
    );

    // TODO: This test should fail once we fix the tree-sitter bug.
    // TODO: It should only go to row: 3, column: 1.
    // `if` statements without an `else` don't consume newlines
    statement_range_test(
"
<<if @(a > b) {
    1 + 1
}


>>",
    );

    // Subsetting runs whole expression
    statement_range_test(
"
<<dt[
  a > b,
  by @= 4,
  foo
]>>
",
    );

    // Blocks within calls run one line at a time (testthat, withr, quote())
    statement_range_test(
"
test_that('stuff', {
  <<x @<- 1>>
  y <- 2
  expect_equal(x, y)
})
",
    );

    // But can run entire expression
    statement_range_test(
"
<<test_that(@'stuff', {
  x <- 1
  y <- 2
  expect_equal(x, y)
})>>
",
    );
    statement_range_test(
"
<<test_that('stuff', {
  x <- 1
  y <- 2
  expect_equal(x, y)
}@)>>
",
    );

    // Comments are skipped from root level
    statement_range_test(
"
@
# hi there

# another one

<<1 + 1>>
",
    );

    // Comments are skipped in blocks
    statement_range_test(
"
{
    # hi there@

    # another one

    <<1 + 1>>
}
",
    );

    // Unmatched opening braces send the full partial statement
    statement_range_test(
"
@
<<{
    1 + 1

>>",
    );

    // Binary op with braces respects that you can put the cursor inside the braces
    statement_range_test(
"
1 + {
    <<2 + 2@>>
}
",
    );

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
