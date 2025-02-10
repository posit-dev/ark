use ropey::Rope;
use tree_sitter::Node;

use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::node_find_parent_call;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

static NO_PARAMETER_HINTS_FUNCTIONS: &[&str] =
    &["args", "debug", "debugonce", "help", "trace", "str"];

#[derive(Debug, Copy, Clone)]
pub(crate) enum ParameterHints {
    Enabled,
    Disabled,
}

impl ParameterHints {
    pub(crate) fn is_enabled(&self) -> bool {
        matches!(self, ParameterHints::Enabled)
    }
}

/// If we end up providing function completions for [Node], should those function
/// completions automatically add `()` and trigger Parameter Hints?
///
/// The answer is always yes, except for:
/// - When we are inside special functions, like `debug(acro<>)` or `debugonce(dplyr::acr<>)`
/// - When we are inside `?`, like `?acr<>` or `method?acr<>`
pub(crate) fn parameter_hints(node: Node, contents: &Rope) -> ParameterHints {
    if is_inside_no_parameter_hints_function(node, contents) {
        return ParameterHints::Disabled;
    }

    if is_inside_help(node) {
        return ParameterHints::Disabled;
    }

    ParameterHints::Enabled
}

fn is_inside_no_parameter_hints_function(node: Node, contents: &Rope) -> bool {
    // For `debug(pkg::fn<>)`, use `pkg::fn` as the relevant node.
    // For `debug(pkg::<>)`, use `pkg::` as the relevant node.
    let node = skip_namespace_operator(node);

    // Assuming we are completing a call argument, find the containing call node
    let Some(node) = node_find_parent_call(&node) else {
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

    // TODO: The situation re binary help is a bit more complex, as there are
    // cases where you probably *do* want to add parentheses, such as
    // `method?show("numeric")`. We can revisit this in future.
    // Good reference for these S4 matters: ?methods::Documentation

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

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::CompletionItem;
    use tower_lsp::lsp_types::InsertTextFormat;
    use tree_sitter::Point;

    use crate::lsp::completions::provide_completions;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::lsp::state::WorldState;
    use crate::r_task;

    fn point_from_cursor(text: &str) -> (String, Point) {
        let cursor_pos = text.find('@').unwrap();
        let text = text.replace('@', "");
        (text, Point::new(0, cursor_pos))
    }

    fn find_completion(completions: &[CompletionItem], label: &str) -> CompletionItem {
        completions
            .iter()
            .find(|item| item.label == label)
            .unwrap()
            .clone()
    }

    #[test]
    fn test_completions_dont_add_parentheses_inside_special_functions_naked() {
        r_task(|| {
            let (text, point) = point_from_cursor("debug(enc@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &WorldState::default()).unwrap();
            let completion = find_completion(&completions, "enc2native");

            // (1) correct string (no trailing parens)
            // (2) plain text, not a snippet with a placeholder for the cursor
            // (3) no extra command to trigger parameter hints
            assert_eq!(completion.insert_text.unwrap(), String::from("enc2native"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_inside_special_functions_double_colon() {
        r_task(|| {
            let state = WorldState::default();

            let (text, point) = point_from_cursor("debug(base::ab@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &state).unwrap();
            let completion = find_completion(&completions, "abs");

            assert_eq!(completion.insert_text.unwrap(), String::from("abs"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());

            // User hasn't typed any namespace name yet, but we show them a completion list
            // here and they pick from it, so it's a common case
            let (text, point) = point_from_cursor("debug(base::@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &state).unwrap();
            let completion = find_completion(&completions, "abs");

            assert_eq!(completion.insert_text.unwrap(), String::from("abs"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_inside_special_functions_triple_colon() {
        r_task(|| {
            let (text, point) = point_from_cursor("debug(utils:::.get@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &WorldState::default()).unwrap();
            let completion = find_completion(&completions, ".getHelpFile");

            assert_eq!(
                completion.insert_text.unwrap(),
                String::from(".getHelpFile")
            );
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_for_help_operator_naked() {
        r_task(|| {
            let state = WorldState::default();

            // Unary help
            let (text, point) = point_from_cursor("?enc@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &state).unwrap();
            let completion = find_completion(&completions, "enc2native");

            assert_eq!(completion.insert_text.unwrap(), String::from("enc2native"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());

            // Binary help
            let (text, point) = point_from_cursor("methods?ini@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &state).unwrap();
            let completion = find_completion(&completions, "initialize");

            assert_eq!(completion.insert_text.unwrap(), String::from("initialize"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_for_help_operator_double_colon() {
        r_task(|| {
            let state = WorldState::default();

            // Unary help
            let (text, point) = point_from_cursor("?base::enc@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &state).unwrap();
            let completion = find_completion(&completions, "enc2native");

            assert_eq!(completion.insert_text.unwrap(), String::from("enc2native"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());

            // Binary help
            let (text, point) = point_from_cursor("methods?methods::sho@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &state).unwrap();
            let completion = find_completion(&completions, "show");

            assert_eq!(completion.insert_text.unwrap(), String::from("show"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());

            // User hasn't typed any namespace name yet, but we show them a completion list
            // here and they pick from it, so it's a common case
            let (text, point) = point_from_cursor("?base::@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &state).unwrap();
            let completion = find_completion(&completions, "enc2native");

            assert_eq!(completion.insert_text.unwrap(), String::from("enc2native"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }
}
