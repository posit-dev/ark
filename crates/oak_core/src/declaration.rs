//! Helpers for detecting `declare()` annotations in R source code.
//!
//! `declare()` is a no-op function in R (>= 4.5) meant to hold static
//! annotations. The compat syntax uses `~declare(...)` (a formula, also a
//! no-op) for older R versions.
//!
//! This module recognises the `declare()` wrapper and returns its arguments for
//! the caller to interpret.

use aether_syntax::AnyRExpression;
use aether_syntax::RCall;
use aether_syntax::RCallArguments;
use aether_syntax::RSyntaxKind;

use crate::syntax_ext::RIdentifierExt;

/// If `expr` is `declare(...)` or `~declare(...)`, return the arguments
/// of the `declare()` call. Returns `None` if the expression doesn't
/// match either pattern.
pub fn as_declare_args(expr: &AnyRExpression) -> Option<RCallArguments> {
    let call = as_declare_call(expr)?;
    call.arguments().ok()
}

/// Unwrap `declare(...)` or `~declare(...)` to get the `declare` call node.
fn as_declare_call(expr: &AnyRExpression) -> Option<RCall> {
    match expr {
        AnyRExpression::RCall(call) if is_declare(call) => Some(call.clone()),

        AnyRExpression::RUnaryExpression(unary) => {
            let op = unary.operator().ok()?;
            if op.kind() != RSyntaxKind::TILDE {
                return None;
            }
            let AnyRExpression::RCall(call) = unary.argument().ok()? else {
                return None;
            };
            if is_declare(&call) {
                Some(call)
            } else {
                None
            }
        },

        _ => None,
    }
}

fn is_declare(call: &RCall) -> bool {
    let Ok(AnyRExpression::RIdentifier(ident)) = call.function() else {
        return false;
    };
    ident.name_text() == "declare"
}

#[cfg(test)]
mod tests {
    use aether_parser::RParserOptions;
    use aether_syntax::AnyRExpression;
    use biome_rowan::AstNode;
    use biome_rowan::AstNodeList;
    use biome_rowan::AstSeparatedList;

    use super::*;

    fn parse_single_expr(code: &str) -> AnyRExpression {
        let parsed = aether_parser::parse(code, RParserOptions::default());
        parsed.tree().expressions().iter().next().unwrap()
    }

    fn declare_arg_values(code: &str) -> Option<Vec<String>> {
        let expr = parse_single_expr(code);
        let args = as_declare_args(&expr)?;
        Some(
            args.items()
                .iter()
                .filter_map(|arg| {
                    let arg = arg.ok()?;
                    Some(arg.value()?.syntax().text_trimmed().to_string())
                })
                .collect(),
        )
    }

    #[test]
    fn test_declare_returns_arguments() {
        let values = declare_arg_values("declare(source(\"helpers.R\"))");
        assert_eq!(values, Some(vec!["source(\"helpers.R\")".to_string()]));
    }

    #[test]
    fn test_tilde_declare_returns_arguments() {
        let values = declare_arg_values("~declare(source(\"helpers.R\"))");
        assert_eq!(values, Some(vec!["source(\"helpers.R\")".to_string()]));
    }

    #[test]
    fn test_bare_call_not_declare() {
        let values = declare_arg_values("source(\"helpers.R\")");
        assert_eq!(values, None);
    }

    #[test]
    fn test_tilde_not_declare() {
        let values = declare_arg_values("~other(source(\"helpers.R\"))");
        assert_eq!(values, None);
    }

    #[test]
    fn test_declare_no_args() {
        let values = declare_arg_values("declare()");
        assert_eq!(values, Some(vec![]));
    }

    #[test]
    fn test_declare_multiple_args() {
        let values = declare_arg_values("declare(source(\"a.R\"), source(\"b.R\"))");
        assert_eq!(
            values,
            Some(vec![
                "source(\"a.R\")".to_string(),
                "source(\"b.R\")".to_string(),
            ])
        );
    }

    #[test]
    fn test_declare_preserves_named_args() {
        let expr = parse_single_expr("declare(foo = source(\"a.R\"))");
        let args = as_declare_args(&expr).unwrap();
        let arg = args.items().iter().next().unwrap().unwrap();
        assert!(arg.name_clause().is_some());
    }
}
