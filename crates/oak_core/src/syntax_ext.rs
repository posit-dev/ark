use aether_syntax::AnyRExpression;
use aether_syntax::AnyRSelector;
use aether_syntax::AnyRStatement;
use aether_syntax::RCall;
use aether_syntax::RIdentifier;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use aether_syntax::RSyntaxToken;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::TextSize;
use biome_rowan::TokenAtOffset;

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

/// The last token that starts before `offset`, ignoring trivia.
///
/// Recognition starts here rather than at the token covering the cursor.
/// Air attaches whitespace, newlines and comments to tokens as trivia, so in
/// `f(1, ` followed by a newline at EOF, the cursor token has only the root
/// as an ancestor. The preceding `,` belongs to the unclosed call and keeps
/// its enclosing syntax available for recognition.
pub fn last_token_before(root: &RSyntaxNode, offset: TextSize) -> Option<RSyntaxToken> {
    // The right token of a pair starts at `offset`, so it can't qualify, and
    // later tokens start after it. `None` happens for an empty file.
    let mut token = match root.token_at_offset(offset) {
        TokenAtOffset::None => root.last_token()?,
        TokenAtOffset::Single(token) => token,
        TokenAtOffset::Between(left, _) => left,
    };

    loop {
        let range = token.text_trimmed_range();
        if !range.is_empty() && range.start() < offset {
            return Some(token);
        }
        token = token.prev_token()?;
    }
}

/// Includes identifiers, `...`, `..1`, keywords that can prefix longer names
/// (`for` in `format`), and unclosed backquoted names lexed as `ERROR_TOKEN`.
pub fn is_name_token(token: &RSyntaxToken) -> bool {
    use RSyntaxKind::*;

    match token.kind() {
        IDENT | DOTS | DOTDOTI => true,
        FUNCTION_KW | FOR_KW | IN_KW | WHILE_KW | REPEAT_KW | IF_KW | ELSE_KW | NEXT_KW |
        BREAK_KW | TRUE_KW | FALSE_KW | NULL_KW | INF_KW | NAN_KW | NA_LOGICAL_KW |
        NA_INTEGER_KW | NA_DOUBLE_KW | NA_COMPLEX_KW | NA_CHARACTER_KW => true,
        ERROR_TOKEN => token.text_trimmed().starts_with('`'),
        _ => false,
    }
}

/// Whether `offset` is inside or at the end of `token`, the condition for a
/// name token to supply typed text.
pub fn touches(token: &RSyntaxToken, offset: TextSize) -> bool {
    let range = token.text_trimmed_range();
    range.start() < offset && offset <= range.end()
}

/// Whether `offset` lies between the opening and closing delimiters among
/// `node`'s own tokens, as in `R_CALL_ARGUMENTS`, `R_BRACED_STATEMENTS` or
/// the condition of `R_IF_STATEMENT`. A missing closing delimiter, as in
/// `f(x` at the end of a file, leaves the construct open to the end.
///
/// In `f()|` the last token's ancestors include the call, but the cursor is
/// after its closing delimiter and therefore outside the call.
pub fn delimiters_contain(node: &RSyntaxNode, offset: TextSize) -> bool {
    use RSyntaxKind::*;

    let mut tokens = node
        .children_with_tokens()
        .filter_map(|child| child.into_token());

    let Some(open) =
        tokens.find(|token| matches!(token.kind(), L_PAREN | L_BRACK | L_BRACK2 | L_CURLY))
    else {
        return false;
    };
    let close_kind = match open.kind() {
        L_PAREN => R_PAREN,
        L_BRACK => R_BRACK,
        L_BRACK2 => R_BRACK2,
        _ => R_CURLY,
    };

    if offset < open.text_trimmed_range().end() {
        return false;
    }

    match tokens.find(|token| token.kind() == close_kind) {
        Some(close) => offset <= close.text_trimmed_range().start(),
        None => true,
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
