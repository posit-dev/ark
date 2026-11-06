use aether_syntax::AnyRExpression;
use aether_syntax::AnyRSelector;
use aether_syntax::AnyRStatement;
use aether_syntax::RCall;
use aether_syntax::RIdentifier;
use aether_syntax::RStringValue;
use aether_syntax::RSyntaxKind;
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

pub trait RStringValueExt {
    /// Return the string contents without surrounding delimiters.
    ///
    /// TODO(air): Replace with `RStringValue::inner_string_text()` once Air is
    /// fully updated.
    fn string_text(&self) -> Option<String>;
}

impl RStringValueExt for RStringValue {
    fn string_text(&self) -> Option<String> {
        let token = self.value().ok()?;
        let text = token.text_trimmed();

        match token.kind() {
            RSyntaxKind::R_DOUBLE_QUOTED_STRING_LITERAL => {
                Some(text.strip_prefix('"')?.strip_suffix('"')?.to_string())
            },
            RSyntaxKind::R_SINGLE_QUOTED_STRING_LITERAL => {
                Some(text.strip_prefix('\'')?.strip_suffix('\'')?.to_string())
            },
            RSyntaxKind::R_RAW_STRING_LITERAL => Some(raw_string_inner(text)?.to_string()),
            _ => None,
        }
    }
}

fn raw_string_inner(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(['r', 'R'])?;

    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let rest = &rest[quote.len_utf8()..];

    let dashes = rest.len() - rest.trim_start_matches('-').len();
    let rest = &rest[dashes..];

    let close_bracket = match rest.chars().next()? {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let rest = &rest[1..];

    let rest = rest.strip_suffix(quote)?;
    let rest = rest.strip_suffix(&"-".repeat(dashes))?;
    rest.strip_suffix(close_bracket)
}

pub trait AnyRSelectorExt {
    /// Uses [RIdentifierExt::name_text()] and [RStringValueExt::string_text()] to report
    /// the name without backticks or quotes
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
            AnyRSelector::RStringValue(node) => node.string_text(),
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
            assert_eq!(s.string_text().unwrap(), "hello");
        });
    }

    #[test]
    fn string_single_quoted() {
        assert_matches!(parse_single_expr("'world'"), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.string_text().unwrap(), "world");
        });
    }

    #[test]
    fn string_raw() {
        assert_matches!(parse_single_expr("r\"(hello)\""), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.string_text().unwrap(), "hello");
        });
    }

    #[test]
    fn string_raw_with_dashes_and_brackets() {
        assert_matches!(parse_single_expr("R'--[a)-\"b]--'"), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.string_text().unwrap(), "a)-\"b");
        });
    }

    #[test]
    fn string_raw_braces() {
        assert_matches!(parse_single_expr("r\"{x}\""), AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            assert_eq!(s.string_text().unwrap(), "x");
        });
    }
}
