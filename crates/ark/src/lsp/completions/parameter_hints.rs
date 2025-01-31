use ropey::Rope;
use tree_sitter::Node;

use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::node_find_containing_call;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

static NO_PARAMETER_HINTS_FUNCTIONS: &[&str] =
    &["args", "debug", "debugonce", "help", "trace", "str"];

/// If we end up providing function completions for [Node], should those function
/// completions automatically add `()` and trigger Parameter Hints?
///
/// The answer is always yes, except for:
/// - When we are inside special functions, like `debug(acro<>)` or `debugonce(dplyr::acr<>)`
/// - When we are inside `?`, like `?acr<>` or `method?acr<>`
pub(crate) fn parameter_hints(node: Node, contents: &Rope) -> bool {
    if is_inside_no_parameter_hints_function(node, contents) {
        return false;
    }

    if is_inside_help(node) {
        return false;
    }

    true
}

fn is_inside_no_parameter_hints_function(node: Node, contents: &Rope) -> bool {
    // For `debug(pkg::fn<>)`, use `pkg::fn` as the relevant node.
    // For `debug(pkg::<>)`, use `pkg::` as the relevant node.
    let node = skip_namespace_operator(node);

    // Assuming we are completing a call argument, find the containing call node
    let Some(node) = node_find_containing_call(&node) else {
        return false;
    };

    // Pull the call's `"function"` slot
    let Some(node) = node.child_by_field_name("function") else {
        return false;
    };

    // Extract the function name text
    let Ok(text) = contents.node_slice(&node) else {
        return false;
    };

    let text = text.to_string();

    NO_PARAMETER_HINTS_FUNCTIONS.contains(&text.as_str())
}

fn is_inside_help(node: Node) -> bool {
    // For `?pkg::fn<>`, use `pkg::fn` as the relevant node.
    // For `?pkg::<>`, use `pkg::` as the relevant node.
    let node = skip_namespace_operator(node);

    let Some(parent) = node.parent() else {
        return false;
    };

    matches!(
        parent.node_type(),
        NodeType::UnaryOperator(UnaryOperatorType::Help) |
            NodeType::BinaryOperator(BinaryOperatorType::Help)
    )
}

/// Finds the relevant base node for parameter hint analysis
///
/// In the case of providing completions for `fn` with `pkg::fn<>`, the relevant node
/// to start analysis from is `pkg::fn`.
///
/// Similarly, in the case of `pkg::<>` where the user hasn't typed anything else yet,
/// the relevant node to start from is still `pkg::`.
fn skip_namespace_operator(node: Node) -> Node {
    let Some(parent) = node.parent() else {
        return node;
    };

    if parent.is_namespace_operator() {
        parent
    } else {
        node
    }
}
