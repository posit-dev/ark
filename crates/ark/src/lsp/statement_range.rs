//
// statement_range.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use anyhow::bail;
use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde::Serialize;
use stdext::unwrap;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::backend::LspResult;
use crate::lsp::document::Document;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::node_has_error_or_missing;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub static POSITRON_STATEMENT_RANGE_REQUEST: &'static str = "positron/textDocument/statementRange";

// ---------------------------------------------------------------------------------------
// LSP facing types (these use LSP `Range`)

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRangeParams {
    /// The document to provide a statement range for.
    pub text_document: VersionedTextDocumentIdentifier,
    /// The location of the cursor.
    pub position: Position,
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum StatementRangeResponse {
    Success(StatementRangeSuccess),
    Rejection(StatementRangeRejection),
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRangeSuccess {
    /// The document range the statement covers.
    range: lsp_types::Range,
    /// Optionally, code to be executed for this `range` if it differs from
    /// what is actually pointed to by the `range` (i.e. roxygen examples).
    code: Option<String>,
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "rejectionKind")]
pub enum StatementRangeRejection {
    Syntax(StatementRangeSyntaxRejection),
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRangeSyntaxRejection {
    line: Option<u32>,
}

// ---------------------------------------------------------------------------------------
// Internal types (these use tree-sitter `Range`)

#[derive(Debug, Eq, PartialEq)]
enum ArkStatementRangeResponse {
    Success(ArkStatementRangeSuccess),
    Rejection(ArkStatementRangeRejection),
}

#[derive(Debug, Eq, PartialEq)]
struct ArkStatementRangeSuccess {
    range: tree_sitter::Range,
    code: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
enum ArkStatementRangeRejection {
    Syntax(ArkStatementRangeSyntaxRejection),
}

#[derive(Debug, Eq, PartialEq)]
struct ArkStatementRangeSyntaxRejection {
    line: u32,
}

impl ArkStatementRangeResponse {
    // Sole conversion method between `ArkStatementRangeResponse` and `StatementRangeResponse`,
    // which handles all Tree-sitter to LSP conversion at the method boundary
    fn into_lsp_response(self, document: &Document) -> anyhow::Result<StatementRangeResponse> {
        match self {
            ArkStatementRangeResponse::Success(response) => {
                // Tree-sitter `Point`s to LSP `Position`s
                let start =
                    document.lsp_position_from_tree_sitter_point(response.range.start_point)?;
                let end = document.lsp_position_from_tree_sitter_point(response.range.end_point)?;
                let range = lsp_types::Range { start, end };
                Ok(StatementRangeResponse::Success(StatementRangeSuccess {
                    range,
                    code: response.code,
                }))
            },
            ArkStatementRangeResponse::Rejection(rejection) => match rejection {
                ArkStatementRangeRejection::Syntax(rejection) => {
                    Ok(StatementRangeResponse::Rejection(
                        StatementRangeRejection::Syntax(StatementRangeSyntaxRejection {
                            line: Some(rejection.line),
                        }),
                    ))
                },
            },
        }
    }
}

// ---------------------------------------------------------------------------------------

// `Regex::new()` is fairly slow to compile.
// roxygen2 comments can contain 1 or more leading `#` before the `'`.
static RE_ROXYGEN2_COMMENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#+'").unwrap());

pub(crate) fn statement_range(
    document: &Document,
    point: Point,
) -> LspResult<Option<StatementRangeResponse>> {
    let root = document.ast.root_node();
    let contents = &document.contents;

    // Initial check to see if we are in a roxygen2 comment, in which case we parse a
    // subdocument containing the `@examples` or `@examplesIf` section and locate a
    // statement range within that to execute. The returned `code` represents the
    // statement range's code stripped of `#'` tokens so it is runnable.
    if let Some(response) = find_roxygen_statement_range(&root, contents, point)? {
        return Ok(Some(response.into_lsp_response(document)?));
    }

    if let Some(response) = find_statement_range(&root, point.row)? {
        return Ok(Some(response.into_lsp_response(document)?));
    }

    Ok(None)
}

fn find_roxygen_statement_range(
    root: &Node,
    contents: &str,
    point: Point,
) -> LspResult<Option<ArkStatementRangeResponse>> {
    // Refuse to look for roxygen comments in the face of parse errors
    // (posit-dev/positron#5023)
    if node_has_error_or_missing(root) {
        return Ok(None);
    }

    // Find first node that is at or extends past the `point`
    let mut cursor = root.walk();
    if !cursor.goto_first_child_for_point_patched(point) {
        return Ok(None);
    }
    let node = cursor.node();

    // If we are within `@examples` or `@examplesIf`, first find the range that spans the
    // full examples section
    if let Some(range) = find_roxygen_examples_section(node, contents) {
        // Then narrow in on the exact range of code that the user's cursor covers
        if let Some(response) = find_roxygen_examples_range(&root, range, contents, point)? {
            return Ok(Some(response));
        };
    }

    // If we aren't in an `@examples` or `@examplesIf` section (or we were, but it is
    // somehow invalid), we still check to see if we are in a roxygen2 comment of any
    // kind. If so, we send just the current comment line to prevent "jumping" past the
    // entire roxygen block if you misplace your cursor and `Cmd + Enter`.
    if as_roxygen_comment_text(&node, contents).is_some() {
        return Ok(Some(ArkStatementRangeResponse::Success(
            ArkStatementRangeSuccess {
                range: node.range(),
                code: None,
            },
        )));
    }

    // Otherwise we let someone else handle the statement range
    Ok(None)
}

fn as_roxygen_comment_text(node: &Node, contents: &str) -> Option<String> {
    // Tree sitter doesn't know about the special `#'` marker,
    // but does tell us if we are in a `#` comment
    if !node.is_comment() {
        return None;
    }

    let text = node.node_to_string(contents).ok()?;

    // Does the roxygen2 prefix exist?
    if !RE_ROXYGEN2_COMMENT.is_match(&text) {
        return None;
    }

    Some(text)
}

fn find_roxygen_examples_section(node: Node, contents: &str) -> Option<tree_sitter::Range> {
    // Check that the `node` we start on is a valid roxygen comment line.
    // We check this `node` specially because the loops below start on the previous/next
    // sibling, and this one would go unchecked.
    let Some(text) = as_roxygen_comment_text(&node, contents) else {
        return None;
    };

    // Drop `#'` from the comment's text
    let text = RE_ROXYGEN2_COMMENT.replace(&text, "");

    // Trim leading whitespace after the `#'`
    let text = text.trim_start();

    // Do we happen to be on an `@` tag line already?
    // (This is a rough heuristic that we are starting a roxygen tag line. To be more
    // robust we'd need a full roxygen2 parser.)
    if text.starts_with("@") {
        return None;
    }

    let mut last_sibling = node;
    let mut start = None;

    // Walk "up" the page
    //
    // Goal is to find the `@examples` or `@examplesIf` section above us. The line
    // right after that is the `start` node.
    //
    // Note: Cleaner to use `cursor.goto_prev_sibling()` but that seems to have
    // a bug in it (it gets the `kind()` right, but `utf8_text()` returns off by
    // one results).
    while let Some(sibling) = last_sibling.prev_sibling() {
        // Have we exited roxygen comments?
        let Some(sibling_text) = as_roxygen_comment_text(&sibling, contents) else {
            break;
        };

        // Drop `#'` from the comment's text
        let sibling_text = RE_ROXYGEN2_COMMENT.replace(&sibling_text, "");

        // Trim leading whitespace after the `#'`
        let sibling_text = sibling_text.trim_start();

        // Did we discover a new tag?
        if sibling_text.starts_with("@") {
            // If that new tag is `@examples` or `@examplesIf`, save the `last_sibling`
            // right before we found `@examples` or `@examplesIf`. That's the start of
            // our node range.
            if sibling_text.starts_with("@examples") || sibling_text.starts_with("@examplesIf") {
                start = Some(last_sibling);
            }

            break;
        }

        last_sibling = sibling;
    }

    let Some(start) = start else {
        // No `@examples` or `@examplesIf` found
        return None;
    };

    last_sibling = node;

    // Walk "down" the page
    //
    // Goal is to find the last line in this `@examples` or `@examplesIf` section
    while let Some(sibling) = last_sibling.next_sibling() {
        // Have we exited roxygen comments?
        let Some(sibling_text) = as_roxygen_comment_text(&sibling, contents) else {
            break;
        };

        // Drop `#'` from the comment's text
        let sibling_text = RE_ROXYGEN2_COMMENT.replace(&sibling_text, "");

        // Trim leading whitespace after the `#'`
        let sibling_text = sibling_text.trim_start();

        // Did we discover a new tag?
        if sibling_text.starts_with("@") {
            break;
        }

        last_sibling = sibling;
    }

    let end = last_sibling;

    let range = tree_sitter::Range {
        start_byte: start.start_byte(),
        end_byte: end.end_byte(),
        start_point: start.start_position(),
        end_point: end.end_position(),
    };

    return Some(range);
}

fn find_roxygen_examples_range(
    root: &Node,
    range: tree_sitter::Range,
    contents: &str,
    point: Point,
) -> LspResult<Option<ArkStatementRangeResponse>> {
    // Anchor row that we adjust relative to
    let row_adjustment = range.start_point.row;

    // Slice out the `@examples` or `@examplesIf` code block (with leading roxygen comments)
    let Some(slice) = contents.get(range.start_byte..range.end_byte) else {
        return Ok(None);
    };

    // Trim out leading roxygen comments so we are left with a subdocument of actual code
    let subcontents = slice.to_string();
    let subcontents: Vec<String> = subcontents
        .lines()
        .map(|line| {
            // Trim `#'` and at most 1 leading whitespace character. Don't trim more
            // whitespace because that would trim intentional indentation and whitespace
            // in multiline strings.
            let line = RE_ROXYGEN2_COMMENT.replace(line, "");
            line.strip_prefix(" ")
                .map(str::to_string)
                .unwrap_or_else(|| line.to_string())
        })
        .collect();
    let subcontents = subcontents.join("\n");

    // Parse the subdocument
    let subdocument = Document::new(&subcontents, None);
    let subdocument_root = subdocument.ast.root_node();

    // Adjust original document row to point to the subdocument row so we know where to
    // start our search from within the subdocument
    let subdocument_row = point.row - row_adjustment;

    let Some(subdocument_response) = find_statement_range(&subdocument_root, subdocument_row)?
    else {
        // Nothing to execute in the subdocument
        return Ok(None);
    };

    // Adjust back to original document
    Ok(match subdocument_response {
        ArkStatementRangeResponse::Success(subdocument_statement_range) => {
            adjust_roxygen_examples_success(
                subdocument_statement_range,
                &subdocument,
                root,
                row_adjustment,
            )
            .map(ArkStatementRangeResponse::Success)
        },
        ArkStatementRangeResponse::Rejection(subdocument_rejection) => {
            adjust_roxygen_examples_rejection(subdocument_rejection, row_adjustment)
                .map(ArkStatementRangeResponse::Rejection)
        },
    })
}

fn adjust_roxygen_examples_success(
    subdocument_statement_range: ArkStatementRangeSuccess,
    subdocument: &Document,
    root: &Node,
    row_adjustment: usize,
) -> Option<ArkStatementRangeSuccess> {
    let subdocument_range = subdocument_statement_range.range;

    // Slice out code to execute from the subdocument
    let Some(slice) = subdocument
        .contents
        .get(subdocument_range.start_byte..subdocument_range.end_byte)
    else {
        return None;
    };
    let subdocument_code = slice.to_string();

    // Map the `subdocument_range` that covers the executable code back to a `range`
    // in the original document. This is a rough translation, not an exact one.
    // - Find the comment node that corresponds to the starting row. The start of this
    //   is the start of the range.
    // - Find the comment node that corresponds to the ending row. The end of this is
    //   the end of the range.
    let start_point = tree_sitter::Point {
        row: subdocument_range.start_point.row + row_adjustment,
        column: 0,
    };
    let mut cursor = root.walk();
    if !cursor.goto_first_child_for_point_patched(start_point) {
        return None;
    }
    let start_node = cursor.node();

    let end_point = tree_sitter::Point {
        row: subdocument_range.end_point.row + row_adjustment,
        column: 0,
    };
    let mut cursor = root.walk();
    if !cursor.goto_first_child_for_point_patched(end_point) {
        return None;
    }
    let end_node = cursor.node();

    let range = tree_sitter::Range {
        start_byte: start_node.start_byte(),
        end_byte: end_node.end_byte(),
        start_point: start_node.start_position(),
        end_point: end_node.end_position(),
    };

    let code = Some(subdocument_code);

    Some(ArkStatementRangeSuccess { range, code })
}

fn adjust_roxygen_examples_rejection(
    subdocument_rejection: ArkStatementRangeRejection,
    row_adjustment: usize,
) -> Option<ArkStatementRangeRejection> {
    match subdocument_rejection {
        ArkStatementRangeRejection::Syntax(subdocument_rejection) => {
            // Adjust line number of the syntax rejection to reflect original document
            Some(ArkStatementRangeRejection::Syntax(
                ArkStatementRangeSyntaxRejection {
                    line: subdocument_rejection.line + row_adjustment as u32,
                },
            ))
        },
    }
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

fn find_statement_range(root: &Node, row: usize) -> LspResult<Option<ArkStatementRangeResponse>> {
    let mut cursor = root.walk();

    let children = root.children(&mut cursor);

    let mut node = None;

    for child in children {
        // Refuse to provide a statement range after detecting a top-level `child` with a
        // parse error. We expect that everything before this top-level `child` has parsed
        // correctly, so if the cursor was above this first parse error, we can execute
        // that code fearlessly. After the first parse error, all bets are off. Even if
        // tree-sitter manages to "recover" further down the page, this is highly
        // unpredictable and is sensitive to minor changes in both tree-sitter-r and the
        // user's precise document contents. Instead, we give up if the user's cursor is
        // anywhere past the first parse error, returning a syntax rejection that points
        // to the first line in this child node, which the frontend will show the user
        // (posit-dev/positron#5023, posit-dev/positron#8350).
        if node_has_error_or_missing(&child) {
            let line = child.start_position().row as u32;
            return Ok(Some(ArkStatementRangeResponse::Rejection(
                ArkStatementRangeRejection::Syntax(ArkStatementRangeSyntaxRejection { line }),
            )));
        }

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
            Ok(candidate) => {
                node = candidate;
            },
            Err(error) => {
                log::error!("Failed to find statement range node due to: {error}.");
            },
        }

        break;
    }

    let Some(node) = node else {
        // No statement range node found, possibly no children or some other issue
        return Ok(None);
    };

    let range = expand_range_across_semicolons(node);
    let code = None;

    Ok(Some(ArkStatementRangeResponse::Success(
        ArkStatementRangeSuccess { range, code },
    )))
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
    use insta::assert_snapshot;
    use tower_lsp::lsp_types;
    use tree_sitter::Node;
    use tree_sitter::Parser;
    use tree_sitter::Point;

    use crate::fixtures::point_and_offset_from_cursor;
    use crate::fixtures::point_from_cursor;
    use crate::lsp::document::Document;
    use crate::lsp::statement_range::find_roxygen_statement_range;
    use crate::lsp::statement_range::find_statement_range;
    use crate::lsp::statement_range::ArkStatementRangeRejection;
    use crate::lsp::statement_range::ArkStatementRangeResponse;
    use crate::lsp::statement_range::ArkStatementRangeSuccess;
    use crate::lsp::statement_range::ArkStatementRangeSyntaxRejection;
    use crate::lsp::statement_range::StatementRangeRejection;
    use crate::lsp::statement_range::StatementRangeResponse;
    use crate::lsp::statement_range::StatementRangeSuccess;
    use crate::lsp::statement_range::StatementRangeSyntaxRejection;

    // Intended to ease statement range testing. Supply `x` as a string containing
    // the expression to test along with:
    // - `@` marking the cursor position
    // - `<<` marking the expected selection start position
    // - `>>` marking the expected selection end position
    // These characters will be replaced with the empty string before being parsed
    // by tree-sitter. It is generally best to left align the string against the
    // far left margin to avoid unexpected whitespace and mimic real life.
    #[track_caller]
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

        let response = match find_statement_range(&root, cursor.unwrap().row) {
            Ok(response) => match response {
                Some(response) => response,
                None => panic!("Unexpected `None`"),
            },
            Err(error) => panic!("Unexpected statement range error: {error:?}"),
        };

        let statement_range = match response {
            ArkStatementRangeResponse::Success(statement_range) => statement_range,
            ArkStatementRangeResponse::Rejection(rejection) => {
                panic!("Unexpected statement range rejection: {rejection:?}")
            },
        };

        assert_eq!(
            statement_range.range.start_point,
            sel_start.unwrap(),
            "Failed on test {original}"
        );
        assert_eq!(
            statement_range.range.end_point,
            sel_end.unwrap(),
            "Failed on test {original}"
        );
    }

    // Intended to ease statement range rejection testing. Supply `x` as a string containing
    // the expression to test along with:
    // - `@` marking the cursor position
    // Returns a statement range rejection.
    fn statement_range_rejection_from_cursor(x: &str) -> ArkStatementRangeRejection {
        let (contents, point) = point_from_cursor(x);

        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to create parser");
        let ast = parser.parse(contents, None).unwrap();
        let root = ast.root_node();

        let response = match find_statement_range(&root, point.row) {
            Ok(response) => match response {
                Some(response) => response,
                None => panic!("Unexpected `None`"),
            },
            Err(error) => panic!("Unexpected statement range error: {error:?}"),
        };

        match response {
            ArkStatementRangeResponse::Success(statement_range) => {
                panic!("Unexpected statement range: {statement_range:?}")
            },
            ArkStatementRangeResponse::Rejection(rejection) => rejection,
        }
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
        assert_matches::assert_matches!(find_statement_range(&root, row), Ok(None));
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
        assert_matches::assert_matches!(find_statement_range(&root, row), Ok(None));
    }

    #[test]
    fn test_can_compute_top_level_statement_range_above_first_parse_error() {
        statement_range_test(
            "
@
<<1 + 1>>
sum(
",
        );

        statement_range_test(
            "
<<fn(@
  a,
  b
)>>
sum(
",
        );

        // Before pipeline
        statement_range_test(
            "
@
<<mtcars |>
  mutate()>>
sum(
",
        );

        // On start of pipeline
        statement_range_test(
            "
<<mtcars |>@
  mutate()>>
sum(
",
        );

        // On end of pipeline
        statement_range_test(
            "
<<mtcars |>
  mutate()>>@
sum(
",
        );
    }

    #[test]
    fn test_cant_compute_top_level_statement_range_below_first_parse_error() {
        // Once we hit the very first top-level node containing a parse error, all bets
        // are off. We give up on being able to correctly parse the rest of the file.
        let rejection = statement_range_rejection_from_cursor(
            "
sum(

1 + 1@
",
        );
        assert_matches::assert_matches!(
            rejection,
            ArkStatementRangeRejection::Syntax(ArkStatementRangeSyntaxRejection { line: 1 })
        );
    }

    #[test]
    fn test_cant_compute_top_level_statement_range_below_first_parse_error_even_if_tree_sitter_might_recover(
    ) {
        // Even though the tree-sitter error is somewhat locally contained to `fn` and
        // `fn2` and often will recover before getting to `fn3`, we disallow execution on
        // ANYTHING past the first top-level node containing a parse error for overall
        // predictability of this feature. tree-sitter is just too sensitive to the exact
        // tree-sitter-r grammar, and to the precise contents of the user's document, for
        // post-error recovery to be very useful to us.
        let rejection = statement_range_rejection_from_cursor(
            "
fn <- function) {} # Parse error here
fn2 <- function() {}

fn3 <- function() {@ # Can't execute this!
  1 + 1
}
",
        );
        assert_matches::assert_matches!(
            rejection,
            ArkStatementRangeRejection::Syntax(ArkStatementRangeSyntaxRejection { line: 1 })
        );
    }

    #[test]
    fn test_cant_compute_statement_range_in_child_node_when_parent_contains_any_parse_errors() {
        // Even though `1 + 1` is ABOVE `sum(`, we still can't execute it.
        // We detected that `fn` had at least 1 error node, and gave up at that point
        // before recursing into it. We can't reliably trust tree-sitter enough here to
        // recognize the `{`, step into that, and recurse over its children to recognize
        // that `1 + 1` is above the parse error.
        let rejection = statement_range_rejection_from_cursor(
            "
fn <- function() {
    1 + 1@
    sum(
}
",
        );
        assert_matches::assert_matches!(
            rejection,
            ArkStatementRangeRejection::Syntax(ArkStatementRangeSyntaxRejection { line: 1 })
        );

        let rejection = statement_range_rejection_from_cursor(
            "
{
    1 + 1@
    sum(
}
",
        );
        assert_matches::assert_matches!(
            rejection,
            ArkStatementRangeRejection::Syntax(ArkStatementRangeSyntaxRejection { line: 1 })
        );
    }

    fn get_text(range: tree_sitter::Range, contents: &str) -> String {
        contents
            .get(range.start_byte..range.end_byte)
            .unwrap()
            .to_string()
    }

    // Roxygen comments use the typical `@` cursor token, so we look for `^` instead
    fn statement_range_point_from_cursor(x: &str) -> (String, Point) {
        let (text, point, _offset) = point_and_offset_from_cursor(x, b'^');
        (text, point)
    }

    // We typically want the `.unwrap().unwrap()` behavior during tests because we are
    // testing the actual range and code returned
    fn find_roxygen_statement_range_unsafe(
        root: &Node,
        contents: &str,
        point: Point,
    ) -> ArkStatementRangeSuccess {
        let response = find_roxygen_statement_range(root, contents, point);

        let response = match response {
            Ok(response) => response,
            Err(error) => panic!("Unexpected statement range error: {error:?}"),
        };

        let response = match response {
            Some(response) => response,
            None => panic!("Unexpected `None` in statement range"),
        };

        let statement_range = match response {
            ArkStatementRangeResponse::Success(statement_range) => statement_range,
            ArkStatementRangeResponse::Rejection(rejection) => {
                panic!("Unexpected statement range rejection: {rejection:?}")
            },
        };

        statement_range
    }

    // Useful when testing the rejection case
    fn find_roxygen_statement_range_rejection(
        root: &Node,
        contents: &str,
        point: Point,
    ) -> ArkStatementRangeRejection {
        let response = find_roxygen_statement_range(root, contents, point);

        let response = match response {
            Ok(response) => response,
            Err(error) => panic!("Unexpected statement range error: {error:?}"),
        };

        let response = match response {
            Some(response) => response,
            None => panic!("Unexpected `None` in statement range"),
        };

        let rejection = match response {
            ArkStatementRangeResponse::Success(statement_range) => {
                panic!("Unexpected statement range: {statement_range:?}")
            },
            ArkStatementRangeResponse::Rejection(rejection) => rejection,
        };

        rejection
    }

    #[test]
    fn test_statement_range_roxygen_outside_examples() {
        // Sends just this line's range, we want Positron to "execute" just this line and
        // step forward one line to avoid "jumpiness" if you accidentally send a
        // non-example line to the console
        let text = "
#' ^Hi
#' @param x foo
#' @examples
#' 1 + 1
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' Hi")
        );
        assert!(statement_range.code.is_none());
    }

    #[test]
    fn test_statement_range_roxygen_on_examples() {
        let text = "
#' Hi
#' @param x foo
#' ^@examples
#' 1 + 1
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' @examples")
        );
        assert!(statement_range.code.is_none());
    }

    #[test]
    fn test_statement_range_roxygen_on_examplesif() {
        let text = "
#' Hi
#' @param x foo
#' ^@examplesIf
#' 1 + 1
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' @examplesIf")
        );
        assert!(statement_range.code.is_none());
    }

    #[test]
    fn test_statement_range_roxygen_single_line() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#'^ 1 + 1
#' 2 + 2
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' 1 + 1")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("1 + 1"));
    }

    #[test]
    fn test_statement_range_roxygen_no_examples() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#'^
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#'")
        );
        assert!(statement_range.code.is_none());
    }

    #[test]
    fn test_statement_range_roxygen_no_examples_followed_by_another_tag() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#' ^@returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' @returns")
        );
        assert!(statement_range.code.is_none());
    }

    #[test]
    fn test_statement_range_roxygen_on_multiline_function() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#'
#' fn <- function() {^
#'
#' }
#'
#' 2 + 2
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from(
                "
#' fn <- function() {
#'
#' }
"
                .trim()
            )
        );
        assert_eq!(
            statement_range.code.unwrap(),
            String::from(
                "
fn <- function() {

}
"
                .trim()
            )
        );
    }

    #[test]
    fn test_statement_range_roxygen_before_multiline_function() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#'^
#' fn <- function() {
#'
#' }
#'
#' 2 + 2
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from(
                "
#' fn <- function() {
#'
#' }
"
                .trim()
            )
        );
        assert_eq!(
            statement_range.code.unwrap(),
            String::from(
                "
fn <- function() {

}
"
                .trim()
            )
        );
    }

    #[test]
    fn test_statement_range_roxygen_on_multiline_pipe_chain() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#'
#' x %>%^
#'   this() %>%
#'   that()
NULL
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from(
                "
#' x %>%
#'   this() %>%
#'   that()
"
                .trim()
            )
        );
        assert_eq!(
            statement_range.code.unwrap(),
            String::from(
                "
x %>%
  this() %>%
  that()
"
                .trim()
            )
        );
    }

    #[test]
    fn test_statement_range_roxygen_on_comment() {
        // Skips comment, runs next expression
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#' # ^Comment
#' 2 + 2
#' @returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' 2 + 2")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("2 + 2"));
    }

    #[test]
    fn test_statement_range_roxygen_without_leading_space() {
        // Notice `2 + 2` doesn't have the typical leading whitespace
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#'2 + ^2
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#'2 + 2")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("2 + 2"));
    }

    #[test]
    fn test_statement_range_roxygen_after_examples_section() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1
#' 2 + 2
#' ^@returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' @returns")
        );
        assert!(statement_range.code.is_none());
    }

    #[test]
    fn test_statement_range_roxygen_multiple_leading_hash_signs() {
        let text = "
##' Hi
##' @param x foo
##' @examples
##' 1 + 1^
###' 2 + 2
###' @returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("##' 1 + 1")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("1 + 1"));
    }

    #[test]
    fn test_statement_range_roxygen_multiple_leading_hash_signs_and_multi_line_expression() {
        let text = "
##' Hi
##' @param x foo
##' @examples
##' 1 +^
###' 2 +
##' 3
###' @returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("##' 1 +\n###' 2 +\n##' 3")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("1 +\n2 +\n3"));
    }

    #[test]
    fn test_statement_range_roxygen_multiple_leading_hash_signs_and_non_examples() {
        let text = "
##' Hi
##' ^@param x foo
##' @examples
##' 1 +
###' 2 +
##' 3
###' @returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("##' @param x foo")
        );
        assert!(statement_range.code.is_none());
    }

    #[test]
    fn test_statement_range_roxygen_multiple_spaces_before_the_next_tag() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + 1^
#'     @returns
2 + 2
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' 1 + 1")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("1 + 1"));
    }

    #[test]
    fn test_statement_range_roxygen_can_compute_top_level_statement_range_above_first_parse_error()
    {
        // The function call is "above" the first parse error we get from `sum(`
        let text = "
#' Hi
#' @param x foo
#' @examples
#' fn(
#'   a,^
#'   b
#' )
#' sum(
#' @returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' fn(\n#'   a,\n#'   b\n#' )")
        );
        assert_eq!(
            statement_range.code.unwrap(),
            String::from("fn(\n  a,\n  b\n)")
        )
    }

    #[test]
    fn test_statement_range_roxygen_cant_compute_top_level_statement_range_below_first_parse_error()
    {
        // The function call is "below" the first parse error we get from `sum(`.
        // Error line is adjusted to correspond to original document!
        let text = "
#' Hi
#' @param x foo
#' @examples
#' sum(
#' fn(
#'   a,^
#'   b
#' )
#' @returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let rejection = find_roxygen_statement_range_rejection(&root, contents, point);
        assert_matches::assert_matches!(
            rejection,
            ArkStatementRangeRejection::Syntax(ArkStatementRangeSyntaxRejection { line: 4 })
        );
    }

    #[test]
    fn test_statement_range_roxygen_cant_compute_statement_range_while_on_parse_error() {
        let text = "
#' Hi
#' @param x foo
#' @examples
#' 1 + / 1^
#' @returns
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let rejection = find_roxygen_statement_range_rejection(&root, contents, point);
        assert_matches::assert_matches!(
            rejection,
            ArkStatementRangeRejection::Syntax(ArkStatementRangeSyntaxRejection { line: 4 })
        );
    }

    #[test]
    fn test_statement_range_roxygen_parse_errors_in_parent_document() {
        // If the parent document has parse errors, we don't even try.
        // It's best if the user fixes that first.
        // It's not the job of `find_roxygen_statement_range()` to report anything here.
        let text = "
1 + / 1
#' Hi
#' @param x foo
#' @examples
#' 1 + 1^
#' @returns
2 + 2
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        assert!(find_roxygen_statement_range(&root, contents, point)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_statement_range_roxygen_examplesif_single_line() {
        let text = "
#' Hi
#' @param x foo
#' @examplesIf rlang::is_interactive()
#' 1 ^+ 1
NULL
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' 1 + 1")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("1 + 1"));
    }

    #[test]
    fn test_statement_range_roxygen_examplesif_multi_line() {
        let text = "
#' Hi
#' @param x foo
#' @examplesIf rlang::is_interactive()
#' 1 ^+
#'   1
NULL
";
        let (text, point) = statement_range_point_from_cursor(text);
        let document = Document::new(&text, None);
        let root = document.ast.root_node();
        let contents = &document.contents;
        let statement_range = find_roxygen_statement_range_unsafe(&root, contents, point);
        assert_eq!(
            get_text(statement_range.range, contents),
            String::from("#' 1 +\n#'   1")
        );
        assert_eq!(statement_range.code.unwrap(), String::from("1 +\n  1"));
    }

    #[test]
    fn test_statement_range_serde_json_success() {
        // No code
        let success = StatementRangeResponse::Success(StatementRangeSuccess {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 5,
                    character: 3,
                },
                end: lsp_types::Position {
                    line: 6,
                    character: 4,
                },
            },
            code: None,
        });
        assert_snapshot!(serde_json::to_string_pretty(&success).unwrap());

        // With code
        let success = StatementRangeResponse::Success(StatementRangeSuccess {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 5,
                    character: 3,
                },
                end: lsp_types::Position {
                    line: 6,
                    character: 4,
                },
            },
            code: Some(String::from("1 + 1")),
        });
        assert_snapshot!(serde_json::to_string_pretty(&success).unwrap());
    }

    #[test]
    fn test_statement_range_serde_json_rejection() {
        // Without `line`
        let rejection = StatementRangeResponse::Rejection(StatementRangeRejection::Syntax(
            StatementRangeSyntaxRejection { line: None },
        ));
        assert_snapshot!(serde_json::to_string_pretty(&rejection).unwrap());

        // With `line`
        let rejection = StatementRangeResponse::Rejection(StatementRangeRejection::Syntax(
            StatementRangeSyntaxRejection { line: Some(12) },
        ));
        assert_snapshot!(serde_json::to_string_pretty(&rejection).unwrap());
    }
}
