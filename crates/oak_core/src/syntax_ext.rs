use aether_syntax::AnyRExpression;
use aether_syntax::AnyRSelector;
use aether_syntax::AnyRStatement;
use aether_syntax::RCall;
use aether_syntax::RIdentifier;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;

// Candidates for upstreaming into `aether_syntax`.

pub trait RIdentifierExt {
    /// Return the symbol name, stripping backtick quoting if present.
    ///
    /// Backtick-quoted identifiers like `` `my var` `` are parsed as
    /// `RIdentifier` nodes whose `text_trimmed()` includes the backticks.
    /// The backticks are a quoting mechanism, not part of the symbol name.
    fn name_text(&self) -> String;
}

impl RIdentifierExt for RIdentifier {
    fn name_text(&self) -> String {
        let text = self.syntax().text_trimmed().to_string();
        match text.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
            Some(inner) => inner.to_string(),
            None => text,
        }
    }
}

pub trait AnyRSelectorExt {
    /// Uses [RIdentifierExt::name_text()] and
    /// [aether_syntax::RStringValue::inner_string_text()] to report the name
    /// without backticks or quotes
    ///
    /// Returns [None] for [aether_syntax::AnyRSelector::RDotDotI] and
    /// [aether_syntax::AnyRSelector::RDots], which we don't consider standard
    /// identifiers, and for [aether_syntax::AnyRSelector::RBogusSelector].
    fn identifier_text(&self) -> Option<String>;
}

impl AnyRSelectorExt for AnyRSelector {
    fn identifier_text(&self) -> Option<String> {
        match self {
            AnyRSelector::RIdentifier(node) => Some(node.name_text()),
            AnyRSelector::RStringValue(node) => Some(node.inner_string_text().ok()?.to_string()),
            AnyRSelector::RDots(_) | AnyRSelector::RDotDotI(_) => None,
            AnyRSelector::RBogusSelector(_) => None,
        }
    }
}

pub trait AnyRStatementExt {
    /// The expression of an [aether_syntax::RExpressionStatement].
    ///
    /// Returns [None] for an [aether_syntax::REmptyStatement] (a lone `;`)
    /// and for an expression statement whose expression is missing.
    fn expression(&self) -> Option<AnyRExpression>;
}

impl AnyRStatementExt for AnyRStatement {
    fn expression(&self) -> Option<AnyRExpression> {
        match self {
            AnyRStatement::RExpressionStatement(statement) => statement.expression().ok(),
            AnyRStatement::REmptyStatement(_) => None,
        }
    }
}

pub trait RCallExt {
    /// The value expression of the argument at `position`, counting in call
    /// order. `None` when there's no argument there or it has no value.
    fn argument_value(&self, position: usize) -> Option<AnyRExpression>;
}

impl RCallExt for RCall {
    fn argument_value(&self, position: usize) -> Option<AnyRExpression> {
        self.arguments()
            .ok()?
            .items()
            .iter()
            .nth(position)?
            .ok()?
            .value()
    }
}

#[cfg(test)]
mod tests {
    use aether_parser::RParserOptions;
    use aether_syntax::AnyRExpression;
    use aether_syntax::AnyRValue;
    use assert_matches::assert_matches;
    use biome_rowan::AstNodeList;

    use super::*;

    fn parse_single_expr(code: &str) -> AnyRExpression {
        let parsed = aether_parser::parse(code, RParserOptions::default());
        let statement = parsed.tree().statements().iter().next().unwrap();
        statement.expression().unwrap()
    }

    #[test]
    fn identifier_plain() {
        assert_matches!(parse_single_expr("foo"), AnyRExpression::RIdentifier(ident) => {
            assert_eq!(ident.name_text(), "foo");
        });
    }

    #[test]
    fn identifier_backtick_quoted() {
        assert_matches!(parse_single_expr("`my var`"), AnyRExpression::RIdentifier(ident) => {
            assert_eq!(ident.name_text(), "my var");
        });
    }

    #[test]
    fn string_double_quoted() {
        assert_matches!(parse_single_expr("\"hello\""), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.inner_string_text().unwrap().text(), "hello");
        });
    }

    #[test]
    fn string_single_quoted() {
        assert_matches!(parse_single_expr("'world'"), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.inner_string_text().unwrap().text(), "world");
        });
    }

    #[test]
    fn string_raw() {
        assert_matches!(parse_single_expr("r\"(hello)\""), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.inner_string_text().unwrap().text(), "hello");
        });
    }

    #[test]
    fn string_raw_with_dashes_and_brackets() {
        assert_matches!(parse_single_expr("R'--[a)-\"b]--'"), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.inner_string_text().unwrap().text(), "a)-\"b");
        });
    }

    #[test]
    fn string_raw_braces() {
        assert_matches!(parse_single_expr("r\"{x}\""), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.inner_string_text().unwrap().text(), "x");
        });
    }

    #[test]
    fn string_raw_pipes() {
        assert_matches!(parse_single_expr("r\"-|a|-\""), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.inner_string_text().unwrap().text(), "a");
        });
    }
}
