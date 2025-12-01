//
// function_context.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use stdext::result::ResultExt;
use tower_lsp::lsp_types::Range;
use tree_sitter::Node;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::node_find_parent_call;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

/// Represents how a function is being used in an expression
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FunctionRefUsage {
    /// Function is being called, e.g., `foo()`
    Call,
    /// Function is being referenced as a value without calling, e.g., `foo` in `debug(foo)`
    Value,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionContext {
    /// The name of the function (could be, and often is, a fragment)
    pub name: String,
    /// The LSP range of the function name
    pub range: Range,
    /// How the function is being used (call vs reference)
    pub usage: FunctionRefUsage,
    /// The status of the function's arguments
    pub arguments_status: ArgumentsStatus,
    /// Whether the cursor is at the end of the effective function node
    pub cursor_is_at_end: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ArgumentsStatus {
    /// No arguments node exists, either because it's a function reference or
    /// the arguments just don't exist *yet*
    Absent,
    /// Arguments node exists, but is empty (e.g. `foo()`)
    Empty,
    /// Arguments node exists and has content, even if it's just whitespace
    /// (e.g. `foo(x = "hi")` or `foo(\n  \n)`)
    Nonempty,
}

impl FunctionContext {
    pub(crate) fn new(document_context: &DocumentContext) -> Self {
        let completion_node = document_context.node;

        let Some(effective_function_node) = get_effective_function_node(completion_node) else {
            // We shouldn't ever attempt to instantiate a FunctionContext or
            // function-flavored CompletionItem in this degenerate case, but we
            // return a dummy FunctionContext just to be safe.
            let node_end = document_context
                .document
                .lsp_position_from_tree_sitter_point(completion_node.range().end_point);

            return Self {
                name: String::new(),
                range: tower_lsp::lsp_types::Range::new(node_end, node_end),
                usage: FunctionRefUsage::Call,
                arguments_status: ArgumentsStatus::Absent,
                cursor_is_at_end: true,
            };
        };

        let usage = determine_function_usage(
            &effective_function_node,
            &document_context.document.contents,
        );

        let function_name_node = if effective_function_node.is_namespace_operator() {
            // Note: this could be 'None', in the case of, e.g., `dplyr::@`
            effective_function_node.child_by_field_name("rhs")
        } else {
            Some(effective_function_node)
        };

        let cursor = document_context.point;
        let node_range = effective_function_node.range();
        let is_cursor_at_end =
            cursor.row == node_range.end_point.row && cursor.column == node_range.end_point.column;

        let name = match function_name_node {
            Some(node) => node
                .node_to_string(&document_context.document.contents)
                .log_err()
                .unwrap_or_default(),
            None => String::new(),
        };

        let arguments_status = if usage == FunctionRefUsage::Value {
            ArgumentsStatus::Absent
        } else {
            determine_arguments_status(&effective_function_node)
        };

        log::info!(
            "FunctionContext created with name: '{name}', usage: {usage:?}, arguments: {arguments_status:?}, cursor at end: {is_cursor_at_end}"
        );

        Self {
            name,
            range: match function_name_node {
                Some(node) => document_context
                    .document
                    .lsp_range_from_tree_sitter_range(node.range()),
                None => {
                    // Create a zero-width range at the end of the effective_function_node
                    let node_end = document_context
                        .document
                        .lsp_position_from_tree_sitter_point(
                            effective_function_node.range().end_point,
                        );
                    tower_lsp::lsp_types::Range::new(node_end, node_end)
                },
            },
            usage,
            arguments_status,
            cursor_is_at_end: is_cursor_at_end,
        }
    }
}

/// The practical definition of the effective function node is "Which node
/// should I take the parent of, if I want the parent of a function call or
/// reference?"
///
/// This handles both simple identifiers (`fcn`) and namespace-qualified
/// identifiers (`pkg::fcn`).
///
/// The alleged function node has to either be an identifier or a
/// namespace operator. Otherwise, we return `None`.
fn get_effective_function_node(node: Node) -> Option<Node> {
    let Some(parent) = node.parent() else {
        return None;
    };

    if parent.is_namespace_operator() {
        Some(parent)
    } else if node.is_identifier() || node.is_namespace_operator() {
        Some(node)
    } else {
        None
    }
}

/// When completing a function inside these functions, we treat it as a value
/// reference (don't automatically add parentheses).
static FUNCTIONS_EXPECTING_A_FUNCTION_REFERENCE: &[&str] = &[
    "args",
    "debug",
    "debugonce",
    "formals",
    "help",
    "trace",
    "str",
];

fn is_inside_special_function(node: &Node, contents: &str) -> bool {
    let Some(call_node) = node_find_parent_call(node) else {
        return false;
    };

    let Some(call_name_node) = call_node.child_by_field_name("function") else {
        return false;
    };

    let call_name = call_name_node
        .node_as_str(contents)
        .log_err()
        .unwrap_or_default();

    FUNCTIONS_EXPECTING_A_FUNCTION_REFERENCE.contains(&call_name)
}

/// Checks if the node is inside a help operator context like `?foo` or `method?foo`
fn is_inside_help_operator(node: &Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };

    matches!(
        parent.node_type(),
        NodeType::UnaryOperator(UnaryOperatorType::Help) |
            NodeType::BinaryOperator(BinaryOperatorType::Help)
    )
}

/// - ArgumentsStatus::Empty:    foo()
/// - ArgumentsStatus::Nonempty: foo( )
/// - ArgumentsStatus::Absent:   foo        (not a call, at least not yet)
fn determine_arguments_status(function_container_node: &Node) -> ArgumentsStatus {
    let Some(parent) = function_container_node.parent() else {
        return ArgumentsStatus::Absent;
    };

    if !parent.is_call() {
        return ArgumentsStatus::Absent;
    }

    let Some(arguments_node) = parent.child_by_field_name("arguments") else {
        return ArgumentsStatus::Absent;
    };

    let Some(open_paren) = arguments_node.child_by_field_name("open") else {
        return ArgumentsStatus::Absent;
    };

    let Some(close_paren) = arguments_node.child_by_field_name("close") else {
        return ArgumentsStatus::Absent;
    };

    // Check if "(" is followed immediately by ")"
    if open_paren.end_position().row == close_paren.start_position().row &&
        open_paren.end_position().column == close_paren.start_position().column
    {
        ArgumentsStatus::Empty
    } else {
        ArgumentsStatus::Nonempty
    }
}

fn determine_function_usage(node: &Node, contents: &str) -> FunctionRefUsage {
    if is_inside_special_function(node, contents) || is_inside_help_operator(node) {
        FunctionRefUsage::Value
    } else {
        FunctionRefUsage::Call
    }
}
