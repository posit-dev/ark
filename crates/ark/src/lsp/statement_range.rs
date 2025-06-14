//
// statement_range.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use anyhow::bail;
use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use ropey::Rope;
use serde::Deserialize;
use serde::Serialize;
use stdext::unwrap;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::node_has_error_or_missing;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

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
    pub range: lsp_types::Range,
    /// Optionally, code to be executed for this `range` if it differs from
    /// what is actually pointed to by the `range` (i.e. roxygen examples).
    pub code: Option<String>,
}

// `Regex::new()` is fairly slow to compile.
// roxygen2 comments can contain 1 or more leading `#` before the `'`.
static RE_ROXYGEN2_COMMENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#+'").unwrap());

pub(crate) fn statement_range(
    root: tree_sitter::Node,
    contents: &ropey::Rope,
    point: Point,
    row: usize,
) -> anyhow::Result<Option<StatementRangeResponse>> {
    // Initial check to see if we are in a roxygen2 comment, in which case
    // we exit immediately, returning that line as the `range` and possibly
    // with `code` stripped of the leading `#' ` if we detect that we are in
    // `@examples`.
    if let Some((node, code)) = find_roxygen_comment_at_point(&root, contents, point) {
        let range = node.range();
        return Ok(Some(new_statement_range_response(range, contents, code)));
    }

    if let Some(node) = find_statement_range_node(&root, row) {
        let range = expand_range_across_semicolons(node);
        return Ok(Some(new_statement_range_response(range, contents, None)));
    };

    Ok(None)
}

fn new_statement_range_response(
    range: tree_sitter::Range,
    contents: &Rope,
    code: Option<String>,
) -> StatementRangeResponse {
    // Tree-sitter `Point`s
    let start = range.start_point;
    let end = range.end_point;

    // To LSP `Position`s
    let start = convert_point_to_position(contents, start);
    let end = convert_point_to_position(contents, end);

    let range = lsp_types::Range { start, end };

    StatementRangeResponse { range, code }
}

fn find_roxygen_comment_at_point<'tree>(
    root: &'tree Node,
    contents: &Rope,
    point: Point,
) -> Option<(Node<'tree>, Option<String>)> {
    // Refuse to look for roxygen comments in the face of parse errors
    // (posit-dev/positron#5023)
    if node_has_error_or_missing(root) {
        return None;
    }

    let mut cursor = root.walk();

    // Move cursor to first node that is at or extends past the `point`
    if !cursor.goto_first_child_for_point_patched(point) {
        return None;
    }

    let node = cursor.node();

    // Tree sitter doesn't know about the special `#'` marker,
    // but does tell us if we are in a `#` comment
    if !node.is_comment() {
        return None;
    }

    let text = contents.node_slice(&node).unwrap().to_string();
    let text = text.as_str();

    // Does the roxygen2 prefix exist?
    if !RE_ROXYGEN2_COMMENT.is_match(text) {
        return None;
    }

    let text = RE_ROXYGEN2_COMMENT.replace(text, "").into_owned();

    // It is likely that we have at least 1 leading whitespace character,
    // so we try and remove that if it exists
    let text = match text.strip_prefix(" ") {
        Some(text) => text,
        None => &text,
    };

    // At this point we know we are in a roxygen2 comment block so we are at
    // least going to return this `node` because we run roxygen comments one
    // line at a time (rather than finding the next non-comment node).

    let mut code = None;

    // Do we happen to be on an `@` tag line already?
    // We have to check this specially because the while loop starts with the
    // previous sibling.
    if text.starts_with("@") {
        return Some((node, code));
    }

    // Now look upward to see if we are in an `@examples` section. If we are,
    // then we also return the `code`, which has been stripped of `#' `, so
    // that line can be sent to the console to be executed. This effectively
    // runs roxygen examples in a "dumb" way, 1 line at a time.
    // Note: Cleaner to use `cursor.goto_prev_sibling()` but that seems to have
    // a bug in it (it gets the `kind()` right, but `utf8_text()` returns off by
    // one results).
    let mut last_sibling = node;

    while let Some(sibling) = last_sibling.prev_sibling() {
        last_sibling = sibling;

        // Have we exited comments in general?
        if !sibling.is_comment() {
            break;
        }

        let sibling = contents.node_slice(&sibling).unwrap().to_string();
        let sibling = sibling.as_str();

        // Have we exited roxygen comments specifically?
        if !RE_ROXYGEN2_COMMENT.is_match(sibling) {
            return None;
        }

        let sibling = RE_ROXYGEN2_COMMENT.replace(sibling, "").into_owned();

        // Trim off any leading whitespace
        let sibling = sibling.trim_start();

        // Did we discover that the `node` was indeed in `@examples`?
        if sibling.starts_with("@examples") {
            code = Some(text.to_string());
            break;
        }

        // Otherwise, did we discover the `node` was in a different tag?
        if sibling.starts_with("@") {
            break;
        }
    }

    return Some((node, code));
}

/// Assuming `node` is the first node on a line, `expand_across_semicolons()`
/// checks to see if there are any other non-comment nodes after `node` that
/// share its line number. If there are, that means the nodes are separated by
/// a `;`, and that we should expand the range to also include the node after
/// the `;`.
fn expand_range_across_semicolons(mut node: Node) -> tree_sitter::Range {
    let start_byte = node.start_byte();
    let start_point = node.start_position();

    let mut end_byte = node.end_byte();
    let mut end_point = node.end_position();

    // We know `node` is at the start of a line, but it's possible the node
    // ends with a `;` and needs to be extended
    while let Some(next) = node.next_sibling() {
        let next_start_point = next.start_position();

        if end_point.row != next_start_point.row {
            // Next sibling is on a different row, we are safe
            break;
        }
        if next.is_comment() {
            // Next sibling is a trailing comment, we are safe
            break;
        }

        // Next sibling is on the same line as us, must be separated
        // by a semicolon. Extend end of range to include next sibling.
        end_byte = next.end_byte();
        end_point = next.end_position();

        // Update ending `node` and recheck (i.e. `1; 2; 3`)
        node = next;
    }

    tree_sitter::Range {
        start_byte,
        end_byte,
        start_point,
        end_point,
    }
}

fn find_statement_range_node<'tree>(root: &'tree Node, row: usize) -> Option<Node<'tree>> {
    // Refuse to provide a statement range in the face of parse errors, we are
    // unlikely to be able to provide anything useful, and are more likely to provide
    // something confusing. Instead, return `None` so that the frontend sends code to
    // the console one line at a time (posit-dev/positron#5023).
    if node_has_error_or_missing(root) {
        return None;
    }

    let mut cursor = root.walk();

    let children = root.children(&mut cursor);

    let mut out = None;

    for child in children {
        if row > child.end_position().row {
            // Find the first child who's end position row extends past or is
            // equal to the user selected `row`
            continue;
        }
        if child.is_comment() {
            // Skip comments
            continue;
        }

        // If there was a node associated with the point, recurse into it
        // to figure out exactly which range to select
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
    // If we are on or before the node row, select whole node.
    // End position behavior is node kind dependent.
    if row <= node.start_position().row {
        return Ok(Some(node));
    }

    match node.node_type() {
        NodeType::FunctionDefinition => recurse_function(node, row),
        NodeType::ForStatement | NodeType::WhileStatement | NodeType::RepeatStatement => {
            recurse_loop(node, row)
        },
        NodeType::IfStatement => recurse_if(node, row),
        NodeType::BracedExpression => recurse_braced_expression(node, row),
        NodeType::Subset | NodeType::Subset2 => recurse_subset(node, row),
        NodeType::Call => recurse_call(node, row),
        _ => recurse_default(node, row),
    }
}

fn recurse_function(node: Node, row: usize) -> Result<Option<Node>> {
    let Some(parameters) = node.child_by_field_name("parameters") else {
        bail!("Missing `parameters` field in a `function_definition` node");
    };

    if parameters.start_position().row <= row && parameters.end_position().row >= row {
        // If we are inside the parameters list, select entire function
        // (parameter lists often span multiple lines for long signatures)
        return Ok(Some(node));
    }

    let Some(body) = node.child_by_field_name("body") else {
        // No `body`, select entire function
        return Ok(Some(node));
    };

    if row < body.start_position().row || row > body.end_position().row {
        // We are outside the `body`, so no need to continue recursing
        // (possibly on a newline a user inserted between the parameters and body)
        return Ok(Some(node));
    }

    if body.is_braced_expression() &&
        (row == body.start_position().row || row == body.end_position().row)
    {
        // For the most common `{` bodies, if we are on the `{` or the `}` rows, then we select the
        // entire function. This avoids sending a `{` block without its leading `function` node if
        // `{` happens to be on a different line or if the user is on the `}` line.
        return Ok(Some(node));
    }

    // If we are somewhere inside the body, then we only want to select
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

    if body.is_braced_expression() &&
        (row == body.start_position().row || row == body.end_position().row)
    {
        // For the most common `{` bodies, if we are on the `{` or the `}` rows, then we select the
        // entire loop. This avoids sending a `{` block without its leading loop node if
        // `{` happens to be on a different line or if the user is on the `}` line.
        return Ok(Some(node));
    }

    // If we are somewhere inside the body, then we only want to select
    // the particular expression the cursor is over
    recurse(body, row)
}

fn recurse_if(node: Node, row: usize) -> Result<Option<Node>> {
    let Some(consequence) = node.child_by_field_name("consequence") else {
        bail!("Missing `consequence` child in an `if_statement` node.");
    };
    if row >= consequence.start_position().row && row <= consequence.end_position().row {
        // We are somewhere inside the `consequence`

        if consequence.is_braced_expression() &&
            (row == consequence.start_position().row || row == consequence.end_position().row)
        {
            // On `{` or `}` row of a `{` node, select entire if statement
            return Ok(Some(node));
        }

        // If the `consequence` contains the user row and we aren't on a `{` or `}` row,
        // then we only want to run the expression the cursor is over
        return recurse(consequence, row);
    }

    let Some(alternative) = node.child_by_field_name("alternative") else {
        // No `else` and nothing above matched, select whole if statement
        return Ok(Some(node));
    };
    if row >= alternative.start_position().row && row <= alternative.end_position().row {
        // We are somewhere inside the `alternative`, possibly in an `else if`

        if alternative.is_braced_expression() &&
            (row == alternative.start_position().row || row == alternative.end_position().row)
        {
            // On `{` or `}` row of a `{` node, select entire if statement
            return Ok(Some(node));
        }

        if alternative.is_if_statement() {
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
        bail!("Missing `arguments` field in a `call` node");
    };
    if row == arguments.start_position().row {
        // On start row containing `(`, select whole call
        return Ok(Some(node));
    }
    if row == arguments.end_position().row {
        // On ending row containing `)`, select whole call
        return Ok(Some(node));
    }

    // In general if the user selects a statement while on a function argument,
    // then we select the entire function call. However, calls like
    // test_that("foo", {
    //   1 + 1
    // })
    // purposefully span multiple lines and you want to be able to select
    // `1 + 1` interactively (similar with `withr::local_*()` calls).
    // To detect this we use a heuristic that if the argument `value` node has a
    // different start row than the row you'd get by recursing into that `value`
    // node, then we prefer the row from the recursion, otherwise we select the
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

fn recurse_braced_expression(node: Node, row: usize) -> Result<Option<Node>> {
    if row == node.end_position().row {
        // `recurse()` handled the start position, but if we are on the
        // `}` row, then we also select the entire block
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
        if child.is_comment() {
            // Skip comments
            continue;
        }

        // If there was a node associated with the point, recurse into it
        // to figure out exactly which range to select
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

#[cfg(test)]
mod tests {
    use ropey::Rope;
    use tree_sitter::Node;
    use tree_sitter::Parser;
    use tree_sitter::Point;

    use crate::lsp::statement_range::expand_range_across_semicolons;
    use crate::lsp::statement_range::find_roxygen_comment_at_point;
    use crate::lsp::statement_range::find_statement_range_node;
    use crate::lsp::traits::rope::RopeExt;

    // Intended to ease statement range testing. Supply `x` as a string containing
    // the expression to test along with:
    // - `@` marking the cursor position
    // - `<<` marking the expected selection start position
    // - `>>` marking the expected selection end position
    // These characters will be replaced with the empty string before being parsed
    // by tree-sitter. It is generally best to left align the string against the
    // far left margin to avoid unexpected whitespace and mimic real life.
    fn statement_range_test(x: &str) {
        let original = x;

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
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to create parser");

        let ast = parser.parse(x, None).unwrap();

        let root = ast.root_node();

        let node = find_statement_range_node(&root, cursor.unwrap().row).unwrap();
        let range = expand_range_across_semicolons(node);

        assert_eq!(
            range.start_point,
            sel_start.unwrap(),
            "Failed on test {original}"
        );
        assert_eq!(
            range.end_point,
            sel_end.unwrap(),
            "Failed on test {original}"
        );
    }

    #[test]
    fn test_simple_case() {
        statement_range_test("<<1@+ 1>>");
    }

    #[test]
    fn test_finds_next_row() {
        statement_range_test(
            "
@
<<1 + 1>>
",
        );
    }

    #[test]
    fn test_finds_next_row_with_spaces() {
        statement_range_test(
            "
@



<<1 + 1>>
",
        );
    }

    #[test]
    fn test_selects_all_braces() {
        statement_range_test(
            "
@
<<{
    1 + 1
}>>
",
        );
    }

    #[test]
    fn test_inside_braces_runs_statement_cursor_is_on() {
        statement_range_test(
            "
{
    @<<1 + 1>>
    2 + 2
}
",
        );
    }

    #[test]
    fn test_selects_entire_function() {
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
    }

    #[test]
    fn test_selects_individual_lines_in_function() {
        statement_range_test(
            "
function() {
    1 + 1
    <<2 + @2>>
}
",
        );
    }

    #[test]
    fn test_selects_entire_function_on_multiline_signature() {
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
    }

    #[test]
    fn test_selects_expression_on_one_line_function() {
        statement_range_test(
            "
function()
    @<<1 + 1>>
",
        );
    }

    #[test]
    fn test_selects_expression_on_one_line_function_with_assignment() {
        statement_range_test(
            "
fn <- function()
    @<<1 + 1>>
",
        );
    }

    #[test]
    fn test_selects_entire_function_on_curly_brace_line() {
        statement_range_test(
            "
<<fn <- function()
{@
    1 + 1
}>>
",
        );
    }

    #[test]
    fn test_selects_entire_loop_on_first_or_last_row() {
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
    }

    #[test]
    fn test_runs_line_within_braces_in_loop() {
        statement_range_test(
            "
for(i in 1:5) {
    <<print@(i)>>
    1 + 1
}
",
        );
    }

    #[test]
    fn test_selects_expression_in_one_line_loop_without_braces() {
        statement_range_test(
            "
for(i in 1:5)
    <<print(1)@>>
",
        );
    }

    #[test]
    fn test_selects_entire_loop_on_curly_brace_line() {
        statement_range_test(
            "
<<for(i in 1:5)
{@
    print(1)
}>>
",
        );
    }

    #[test]
    fn test_selects_entire_loop_on_condition_line() {
        statement_range_test(
            "
<<for
(i in @1:5)
{
    1 + 1
}>>
",
        );
    }

    #[test]
    fn test_function_within_function_selects_subfunction() {
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
    }

    #[test]
    fn test_function_with_weird_signature_selects_whole_function() {
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
    }

    #[test]
    fn test_function_with_newlines_runs_whole_function() {
        statement_range_test(
            "
<<function()
@

{
    1 + 1
}>>
",
        );
    }

    #[test]
    fn test_if_statements_run_whole_statement() {
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
    }

    #[test]
    fn test_inside_braces_runs_individual_statements() {
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
    }

    #[test]
    fn test_if_statements_without_braces_should_run_the_whole_if_statement() {
        statement_range_test(
            "
<<if (@a > b)
    1 + 1>>",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
if (a > b)
  <<1 + 1@>>
",
        );
    }

    #[test]
    fn test_top_level_if_else_statements_without_braces_should_run_the_whole_if_statement() {
        statement_range_test(
            "
<<if @(a > b)
  1 + 1 else if (b > c)
  2 + 2 else 4 + 4>>
",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
if (a > b)
  <<@1 + 1 else if (b > c)
  2 + 2 else 4 + 4>>
",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
if (a > b)
  <<1 + 1 else if @(b > c)
  2 + 2 else 4 + 4>>
",
        );

        statement_range_test(
            "
if (a > b)
  1 + 1 else if (b > c)
  <<2 + @2 else 4 + 4>>
",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
if (a > b)
  1 + 1 else if (b > c)
  <<2 + 2 else@ 4 + 4>>
",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
if (a > b)
  1 + 1 else if (b > c)
  <<2 + 2 else 4 @+ 4>>
",
        );
    }

    #[test]
    fn test_if_else_statements_without_braces_but_inside_outer_scope() {
        statement_range_test(
            "
{
    <<if @(a > b)
      1 + 1
    else if (b > c)
      2 + 2
    else
      4 + 4>>
}
",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
{
    if (a > b)
      <<@1 + 1>>
    else if (b > c)
      2 + 2
    else
      4 + 4
}
",
        );

        statement_range_test(
            "
{
    <<if (a > b)
      1 + 1
    else if @(b > c)
      2 + 2
    else
      4 + 4>>
}
",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
{
    if (a > b)
      1 + 1
    else if (b > c)
      <<2 + @2>>
    else
      4 + 4
}
",
        );

        statement_range_test(
            "
{
    <<if (a > b)
      1 + 1
    else if (b > c)
      2 + 2
    else@
      4 + 4>>
}
",
        );

        // TODO: This should run the whole if statement because there are no braces
        statement_range_test(
            "
{
    if (a > b)
      1 + 1
    else if (b > c)
      2 + 2
    else
      <<4 @+ 4>>
}
",
        );
    }

    #[test]
    fn test_if_statements_without_else_dont_consume_newlines() {
        // https://github.com/posit-dev/positron/issues/1464
        statement_range_test(
            "
<<if @(a > b)
    1 + 1>>
",
        );

        statement_range_test(
            "
<<if @(a > b) {
    1 + 1
}>>
",
        );

        statement_range_test(
            "
<<if @(a > b) {
    1 + 1
}>>
if (b > c) {
    2 + 2
}",
        );
    }

    #[test]
    fn test_subsetting_runs_whole_expression() {
        statement_range_test(
            "
<<dt[
  a > b,
  by @= 4,
  foo
]>>
",
        );
    }

    #[test]
    fn test_calls_run_outer_call() {
        statement_range_test(
            "
<<foo(@
  a = 1,
  b
)>>
",
        );

        statement_range_test(
            "
<<foo(
  a = @1,
  b
)>>
",
        );
    }

    #[test]
    fn test_nested_calls_run_outer_call() {
        statement_range_test(
            "
<<foo(bar(
  a = 1,
  b@
))>>
",
        );

        statement_range_test(
            "
<<foo(@bar(
  a = 1,
  b
))>>
",
        );

        // Unless the cursor is within a block, which only runs that line
        statement_range_test(
            "
foo(bar(
  a = {
    <<@1 + 1>>
  },
  b
))
",
        );
    }

    #[test]
    fn test_blocks_within_calls_run_one_line_at_a_time() {
        // testthat, withr, quote()

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
    }

    #[test]
    fn test_comments_are_skipped_from_root_level() {
        statement_range_test(
            "
@
# hi there

# another one

<<1 + 1>>
",
        );
    }

    #[test]
    fn test_comments_are_skipped_in_blocks() {
        statement_range_test(
            "
{
    # hi there@

    # another one

    <<1 + 1>>
}
",
        );
    }

    #[test]
    fn test_binary_op_with_braces_respects_that_you_can_put_the_cursor_inside_the_braces() {
        statement_range_test(
            "
1 + {
    <<2 + 2@>>
}
",
        );
    }

    #[test]
    fn test_multiple_expressions_on_one_line() {
        // https://github.com/posit-dev/positron/issues/4317
        statement_range_test(
            "
<<1@; 2; 3>>
",
        );
        statement_range_test(
            "
<<1; @2; 3>>
",
        );
        statement_range_test(
            "
<<1; 2; 3@>>
",
        );

        // Empty lines don't prevent finding complete lines
        statement_range_test(
            "
@

<<1; 2; 3>>
    ",
        );
    }

    #[test]
    fn test_multiple_expressions_on_one_line_nested_case() {
        statement_range_test(
            "
list({
  @<<1; 2; 3>>
})
    ",
        );
        statement_range_test(
            "
list({
  <<1; @2; 3>>
})
    ",
        );
    }

    #[test]
    fn test_multiple_expressions_after_multiline_expression() {
        // Selects everything
        statement_range_test(
            "
@<<{
  1
}; 2>>
    ",
        );

        // Select up through the second brace
        statement_range_test(
            "
@<<{
  1
}; {
  2
}>>
    ",
        );

        // Only selects `1`
        statement_range_test(
            "
{
  @<<1>>
}; {
  2
}
    ",
        );
    }

    #[test]
    fn test_multiple_expressions_on_one_line_doesnt_select_trailing_comment() {
        statement_range_test(
            "
@<<1>> # trailing
    ",
        );
        statement_range_test(
            "
@<<1; 2>> # trailing
    ",
        );
    }

    #[test]
    fn test_no_top_level_statement() {
        let row = 2;
        let contents = "
1 + 1


";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to create parser");
        let ast = parser.parse(contents, None).unwrap();
        let root = ast.root_node();
        assert_eq!(find_statement_range_node(&root, row), None);
    }

    #[test]
    fn test_no_block_level_statement() {
        let row = 3;
        let contents = "
{
    1 + 1


}
";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to create parser");
        let ast = parser.parse(contents, None).unwrap();
        let root = ast.root_node();
        assert_eq!(find_statement_range_node(&root, row), None);
    }

    #[test]
    fn test_returns_none_with_parse_errors() {
        let row = 2;
        let contents = "
{
    1 + 1
";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to create parser");
        let ast = parser.parse(contents, None).unwrap();
        let root = ast.root_node();
        assert_eq!(find_statement_range_node(&root, row), None);
    }

    #[test]
    fn test_statement_range_roxygen() {
        use crate::lsp::documents::Document;

        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#'
#' fn <- function() {
#'
#' }
#' # Comment
#'2 + 2
#' @returns
";

        let document = Document::new(text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;

        fn get_text(node: &Node, contents: &Rope) -> String {
            contents.node_slice(node).unwrap().to_string()
        }

        // Outside of `@examples`, sends whole line as a comment
        let point = Point { row: 1, column: 2 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, contents), String::from("#' Hi"));
        assert!(code.is_none());

        // On `@examples` line, sends whole line as a comment
        let point = Point { row: 3, column: 2 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, contents), String::from("#' @examples"));
        assert!(code.is_none());

        // At `1 + 1`
        let point = Point { row: 4, column: 2 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, contents), String::from("#' 1 + 1"));
        assert_eq!(code.unwrap(), String::from("1 + 1"));

        // At empty string line after `1 + 1`
        // (we want Positron to trust us and execute this as is)
        let point = Point { row: 5, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, contents), String::from("#'"));
        assert_eq!(code.unwrap(), String::from(""));

        // At `fn <-` line, note we only return that line
        let point = Point { row: 6, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(
            get_text(&node, contents),
            String::from("#' fn <- function() {")
        );
        assert_eq!(code.unwrap(), String::from("fn <- function() {"));

        // At comment line
        let point = Point { row: 9, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, contents), String::from("#' # Comment"));
        assert_eq!(code.unwrap(), String::from("# Comment"));

        // Missing the typical leading space
        let point = Point { row: 10, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, contents), String::from("#'2 + 2"));
        assert_eq!(code.unwrap(), String::from("2 + 2"));

        // At next roxygen tag
        let point = Point { row: 11, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, contents), String::from("#' @returns"));
        assert!(code.is_none());

        let text = "
##' Hi
##' @param x foo
##' @examples
##' 1 + 1
###' 2 + 2
###' @returns
";

        let document = Document::new(text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;

        // With multiple leading `#` followed by code
        let point = Point { row: 4, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, &contents), String::from("##' 1 + 1"));
        assert_eq!(code.unwrap(), String::from("1 + 1"));

        let point = Point { row: 5, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, &contents), String::from("###' 2 + 2"));
        assert_eq!(code.unwrap(), String::from("2 + 2"));

        // With multiple leading `#` followed by non-code
        let point = Point { row: 3, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, &contents), String::from("##' @examples"));
        assert!(code.is_none());

        let point = Point { row: 6, column: 1 };
        let (node, code) = find_roxygen_comment_at_point(&root, contents, point).unwrap();
        assert_eq!(get_text(&node, &contents), String::from("###' @returns"));
        assert!(code.is_none());

        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#' 2 + 2
#' @returns
1 +
";

        let document = Document::new(text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;

        // With parse errors in the file, return `None`
        let point = Point { row: 4, column: 1 };
        assert_eq!(find_roxygen_comment_at_point(&root, contents, point), None);
    }
}
