//
// diagnostics_syntax.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::Diagnostic;
use tree_sitter::Node;
use tree_sitter::Range;

use crate::lsp::diagnostics::DiagnosticContext;
use crate::lsp::encoding::convert_tree_sitter_range_to_lsp_range;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::node_has_error_or_missing;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub(crate) fn syntax_diagnostics(
    root: Node,
    context: &DiagnosticContext,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    recurse(root, context, &mut diagnostics)?;

    Ok(diagnostics)
}

// When we hit an `ERROR` node, i.e. a syntax error, it often has its own children
// which can also be `ERROR`s. The goal is to target the deepest (most precise) `ERROR`
// nodes and only report syntax errors for those. We accomplish this by recursing
// into children first and bailing if we find any children that we reported an error for.
fn recurse(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<bool> {
    if !node_has_error_or_missing(&node) {
        // Stop recursion if this branch of the tree doesn't have issues
        return Ok(false);
    }

    // Always look for contextual `MISSING` issues based on the current node type
    diagnose_missing(node, context, diagnostics)?;

    let mut any_errors = recurse_children(node, context, diagnostics)?;

    // Report an error when:
    // - No children were `ERROR`s
    // - We are an `ERROR`
    if !any_errors && node.is_error() {
        diagnostics.push(syntax_diagnostic(node, context)?);
        any_errors = true;
    }

    Ok(any_errors)
}

fn recurse_children(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<bool> {
    let mut any_errors = false;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        any_errors |= recurse(child, context, diagnostics)?;
    }

    Ok(any_errors)
}

fn syntax_diagnostic(node: Node, context: &DiagnosticContext) -> anyhow::Result<Diagnostic> {
    if let Some(diagnostic) = syntax_diagnostic_missing_open(node, context)? {
        return Ok(diagnostic);
    }

    Ok(syntax_diagnostic_default(node, context))
}

// Use a heuristic that if we see a syntax error and it just contains a `)`, `}`, or `]`,
// then it is probably a case of missing a matching open token.
fn syntax_diagnostic_missing_open(
    node: Node,
    context: &DiagnosticContext,
) -> anyhow::Result<Option<Diagnostic>> {
    let text = context.contents.node_slice(&node)?;

    let open_token = if text == ")" {
        "("
    } else if text == "}" {
        "{"
    } else if text == "]" {
        "["
    } else {
        // Not an unmatched closing token
        return Ok(None);
    };

    let range = node.range();

    Ok(Some(new_missing_open_diagnostic(
        open_token, range, context,
    )))
}

fn syntax_diagnostic_default(node: Node, context: &DiagnosticContext) -> Diagnostic {
    let range = node.range();
    let row_span = range.end_point.row - range.start_point.row;

    if row_span >= 20 {
        return syntax_diagnostic_truncated_default(range, context);
    }

    // The most common case, a localized syntax error that doesn't span too many rows
    let message = String::from("Syntax error");
    new_syntax_diagnostic(message, range, &context)
}

// If the syntax error spans more than 20 rows, just target the starting position
// to avoid overwhelming the user.
fn syntax_diagnostic_truncated_default(range: Range, context: &DiagnosticContext) -> Diagnostic {
    // In theory this is an empty range, as they are constructed like `[ )`, but it
    // seems to work for the purpose of diagnostics, and getting the correct
    // coordinates exactly right seems challenging.
    let start_range = Range {
        start_byte: range.start_byte,
        start_point: range.start_point,
        end_byte: range.start_byte,
        end_point: range.start_point,
    };

    // `+1` because it is user facing and editor UI is 1-indexed
    let end_row = range.end_point.row + 1;
    let message = format!("Syntax error. Starts here and ends on line {end_row}.");

    new_syntax_diagnostic(message, start_range, &context)
}

fn diagnose_missing(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    match node.node_type() {
        NodeType::Parameters => diagnose_missing_parameters(node, context, diagnostics),
        NodeType::BracedExpression => {
            diagnose_missing_braced_expression(node, context, diagnostics)
        },
        NodeType::ParenthesizedExpression => {
            diagnose_missing_parenthesized_expression(node, context, diagnostics)
        },
        NodeType::Call => diagnose_missing_call(node, context, diagnostics),
        NodeType::Subset => diagnose_missing_subset(node, context, diagnostics),
        NodeType::Subset2 => diagnose_missing_subset2(node, context, diagnostics),
        NodeType::BinaryOperator(_) => diagnose_missing_binary_operator(node, context, diagnostics),
        _ => Ok(()),
    }
}

fn diagnose_missing_parameters(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    diagnose_missing_close(node, context, diagnostics, ")")
}

fn diagnose_missing_braced_expression(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    diagnose_missing_close(node, context, diagnostics, "}")
}

fn diagnose_missing_parenthesized_expression(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    diagnose_missing_close(node, context, diagnostics, ")")
}

fn diagnose_missing_call(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    diagnose_missing_call_like(node, context, diagnostics, ")")
}

fn diagnose_missing_subset(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    diagnose_missing_call_like(node, context, diagnostics, "]")
}

fn diagnose_missing_subset2(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    diagnose_missing_call_like(node, context, diagnostics, "]]")
}

fn diagnose_missing_call_like(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
    close_token: &str,
) -> anyhow::Result<()> {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return Ok(());
    };

    diagnose_missing_close(arguments, context, diagnostics, close_token)
}

fn diagnose_missing_binary_operator(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    let Some(rhs) = node.child_by_field_name("rhs") else {
        return Ok(());
    };

    if !rhs.is_missing() {
        // Everything is normal
        return Ok(());
    }

    let Some(operator) = node.child_by_field_name("operator") else {
        return Ok(());
    };

    let range = operator.range();

    let text = context.contents.node_slice(&operator)?;
    let message = format!("Invalid binary operator '{text}'. Missing a right hand side.");

    diagnostics.push(new_syntax_diagnostic(message, range, context));

    Ok(())
}

// For namespace operators, the RHS is actually optional in the grammar,
// to help with autocomplete, so we are looking for when this is `None`.
//
// This means that `dplyr::` is actually "valid" R code according to the
// grammar, so this issue won't ever show up in the syntactic path, even
// though its a syntax problem. Instead we expose it from here and use it
// in the semantic path.
pub(crate) fn diagnose_missing_namespace_operator(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    let None = node.child_by_field_name("rhs") else {
        // Everything is normal
        return Ok(());
    };

    let Some(operator) = node.child_by_field_name("operator") else {
        return Ok(());
    };

    let range = operator.range();

    let text = context.contents.node_slice(&operator)?;
    let message = format!("Invalid namespace operator '{text}'. Missing a right hand side.");

    diagnostics.push(new_syntax_diagnostic(message, range, context));

    Ok(())
}

// `node` must have required `"open"` and `"close"` fields
fn diagnose_missing_close(
    node: Node,
    context: &DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
    close_token: &str,
) -> anyhow::Result<()> {
    let Some(close) = node.child_by_field_name("close") else {
        return Ok(());
    };

    if !close.is_missing() {
        // Everything is normal
        return Ok(());
    }

    let Some(open) = node.child_by_field_name("open") else {
        return Ok(());
    };

    diagnostics.push(new_missing_close_diagnostic(
        close_token,
        open.range(),
        context,
    ));

    Ok(())
}

fn new_missing_open_diagnostic(
    open_token: &str,
    range: Range,
    context: &DiagnosticContext,
) -> Diagnostic {
    let message = format!("Unmatched closing delimiter. Missing an opening '{open_token}'.");
    new_syntax_diagnostic(message, range, context)
}

fn new_missing_close_diagnostic(
    close_token: &str,
    range: Range,
    context: &DiagnosticContext,
) -> Diagnostic {
    let message = format!("Unmatched opening delimiter. Missing a closing '{close_token}'.");
    new_syntax_diagnostic(message, range, context)
}

fn new_syntax_diagnostic(message: String, range: Range, context: &DiagnosticContext) -> Diagnostic {
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    Diagnostic::new_simple(range, message)
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Diagnostic;
    use tower_lsp::lsp_types::Position;

    use crate::lsp::diagnostics::DiagnosticContext;
    use crate::lsp::diagnostics_syntax::syntax_diagnostics;
    use crate::lsp::documents::Document;

    fn text_diagnostics(text: &str) -> Vec<Diagnostic> {
        let document = Document::new(text, None);
        let context = DiagnosticContext::new(&document.contents);
        let diagnostics = syntax_diagnostics(document.ast.root_node(), &context).unwrap();
        diagnostics
    }

    #[test]
    fn test_syntax_error_truncation() {
        // Coded to truncate at 20 rows
        let newlines = "\n".repeat(20);
        let text = String::from("('") + newlines.as_str() + ")";
        let diagnostics = text_diagnostics(text.as_str());
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(0, 0));
        assert_eq!(diagnostic.range.end, Position::new(0, 0));
    }

    #[test]
    fn test_unmatched_call_delimiter() {
        let diagnostics = text_diagnostics("match(a, b");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        assert_eq!(diagnostic.range.start, Position::new(0, 5));
        assert_eq!(diagnostic.range.end, Position::new(0, 6));
        insta::assert_snapshot!(diagnostic.message);

        let diagnostics = text_diagnostics("foo[a, b");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        assert_eq!(diagnostic.range.start, Position::new(0, 3));
        assert_eq!(diagnostic.range.end, Position::new(0, 4));
        insta::assert_snapshot!(diagnostic.message);

        let diagnostics = text_diagnostics("foo[[a, b");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        assert_eq!(diagnostic.range.start, Position::new(0, 3));
        assert_eq!(diagnostic.range.end, Position::new(0, 5));
        insta::assert_snapshot!(diagnostic.message);
    }

    #[test]
    fn test_unmatched_call_delimiter_with_trailing_info() {
        // Expect 2 diagnostics
        // - One about unmatched `(`
        // - But the `)` is implied, meaning that between `2` and `identity` there should be a `,`
        //   so we get a diagnostic for that too
        let text = "
match(1, 2

identity(1)
";
        let diagnostics = text_diagnostics(text);
        assert_eq!(diagnostics.len(), 1);

        // Diagnostic highlights the unmatched `(`
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(1, 5));
        assert_eq!(diagnostic.range.end, Position::new(1, 6));
    }

    #[test]
    fn test_unmatched_braces() {
        let diagnostics = text_diagnostics("{");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);

        let diagnostics = text_diagnostics("{ 1 + 2");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);

        let diagnostics = text_diagnostics("{}");
        assert!(diagnostics.is_empty());

        let diagnostics = text_diagnostics("{ 1 + 2 }");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_unmatched_parentheses() {
        let diagnostics = text_diagnostics("(");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);

        let diagnostics = text_diagnostics("( 1 + 2");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);

        let diagnostics = text_diagnostics("()");
        assert!(diagnostics.is_empty());

        let diagnostics = text_diagnostics("( 1 + 2 )");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_error_precision() {
        // The actual error is up to tree-sitter-r's error recovery,
        // but it should always be decent
        let diagnostics = text_diagnostics("sum(1 * 2 + )");
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(0, 12));
        assert_eq!(diagnostic.range.end, Position::new(0, 13));
    }

    #[test]
    fn test_unmatched_closing_token() {
        let close = vec!["}", ")", "]"];

        for delimiter in close.iter() {
            // i.e. `1 + 1 }`
            let text = format!("1 + 1 {delimiter}");

            let diagnostics = text_diagnostics(text.as_str());
            assert_eq!(diagnostics.len(), 1);

            // Diagnostic highlights the `{delimiter}`
            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
            assert_eq!(diagnostic.range.start, Position::new(0, 6));
            assert_eq!(diagnostic.range.end, Position::new(0, 7));
        }
    }

    #[test]
    fn test_unmatched_closing_token_precision() {
        // Related to https://github.com/tree-sitter/tree-sitter/issues/3623
        // Should target the `}` specifically, not `+ }`.
        let text = "1 + }";
        let diagnostics = text_diagnostics(text);
        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(0, 4));
        assert_eq!(diagnostic.range.end, Position::new(0, 5));
    }

    #[test]
    fn test_unmatched_binary_operator() {
        // The actual error is up to tree-sitter-r's error recovery,
        // but it should always be decent
        let text = "
{
 1 +
}";

        let diagnostics = text_diagnostics(text);
        assert_eq!(diagnostics.len(), 1);

        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(3, 0));
        assert_eq!(diagnostic.range.end, Position::new(3, 1));
    }

    #[test]
    fn test_unmatched_function_parameters_parentheses() {
        // Exact set of diagnostics are up to tree-sitter-r's error recovery,
        // but they should be decent at pointing you to the right place
        let text = "
function(x {
  1 + 1
}";

        let diagnostics = text_diagnostics(text);
        assert_eq!(diagnostics.len(), 2);

        let diagnostic = diagnostics.get(0).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(1, 11));
        assert_eq!(diagnostic.range.end, Position::new(1, 12));

        let diagnostic = diagnostics.get(1).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(3, 0));
        assert_eq!(diagnostic.range.end, Position::new(3, 1));
    }
}
