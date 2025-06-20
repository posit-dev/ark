//
// function_context.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::Range;
use tree_sitter::Node;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::encoding::convert_tree_sitter_range_to_lsp_range;
use crate::treesitter::node_find_parent_call;
use crate::treesitter::node_text;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

/// Represents how a function is being used in an expression
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FunctionUsage {
    /// Function is being called, e.g., `foo()`
    Call,
    /// Function is being referenced without calling, e.g., `foo` in `debug(foo)`
    Reference,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionContext {
    /// The name of the function (could be, and often is, a fragment)
    pub name: String,
    /// The LSP range of the function identifier
    pub range: Range,
    /// How the function is being used (call vs reference)
    pub usage: FunctionUsage,
    /// The status of the function's arguments
    pub arguments_status: ArgumentsStatus,
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
        let function_container_node = skip_namespace_operator(completion_node);

        let function_identifier_node = if function_container_node.is_namespace_operator() {
            match function_container_node.child_by_field_name("rhs") {
                Some(node) => node,
                None => completion_node, // should be impossible
            }
        } else {
            completion_node
        };

        let usage = determine_function_usage(
            &function_container_node,
            &document_context.document.contents,
        );

        let name = node_text(
            &function_identifier_node,
            &document_context.document.contents,
        )
        .unwrap_or_default();

        let arguments_status = if usage == FunctionUsage::Reference {
            ArgumentsStatus::Absent
        } else {
            determine_arguments_status(&function_container_node)
        };

        Self {
            name,
            range: convert_tree_sitter_range_to_lsp_range(
                &document_context.document.contents,
                function_identifier_node.range(),
            ),
            usage,
            arguments_status,
        }
    }
}

pub(crate) fn skip_namespace_operator(node: Node) -> Node {
    let Some(parent) = node.parent() else {
        return node;
    };

    if parent.is_namespace_operator() {
        parent
    } else {
        node
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

fn is_inside_special_function(node: &Node, contents: &ropey::Rope) -> bool {
    let Some(call_node) = node_find_parent_call(node) else {
        return false;
    };

    let Some(call_name_node) = call_node.child_by_field_name("function") else {
        return false;
    };

    let call_name = node_text(&call_name_node, contents).unwrap_or_default();

    FUNCTIONS_EXPECTING_A_FUNCTION_REFERENCE.contains(&call_name.as_str())
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

    let arguments_node = match parent.child_by_field_name("arguments") {
        Some(node) => node,
        None => return ArgumentsStatus::Absent,
    };

    let open_paren = match arguments_node.child_by_field_name("open") {
        Some(node) => node,
        None => return ArgumentsStatus::Absent,
    };

    let close_paren = match arguments_node.child_by_field_name("close") {
        Some(node) => node,
        None => return ArgumentsStatus::Absent,
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

fn determine_function_usage(node: &Node, contents: &ropey::Rope) -> FunctionUsage {
    if is_inside_special_function(node, contents) || is_inside_help_operator(node) {
        FunctionUsage::Reference
    } else {
        FunctionUsage::Call
    }
}
