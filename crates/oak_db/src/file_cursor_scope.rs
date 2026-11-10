use aether_syntax::RBinaryExpression;
use aether_syntax::RBracedStatements;
use aether_syntax::RCallArguments;
use aether_syntax::RElseClause;
use aether_syntax::RExtractExpression;
use aether_syntax::RForStatement;
use aether_syntax::RFunctionDefinition;
use aether_syntax::RIfStatement;
use aether_syntax::RNamespaceExpression;
use aether_syntax::RParameters;
use aether_syntax::RParenthesizedExpression;
use aether_syntax::RRepeatStatement;
use aether_syntax::RSubset2Arguments;
use aether_syntax::RSubsetArguments;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use aether_syntax::RSyntaxToken;
use aether_syntax::RUnaryExpression;
use aether_syntax::RWhileStatement;
use biome_rowan::AstNode;
use biome_rowan::SyntaxResult;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use biome_rowan::TokenAtOffset;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::ScopeId;

use crate::Db;
use crate::File;

impl File {
    /// The scope a cursor at `offset` is in. Position-specific queries such as
    /// [`File::imports_at()`] read the scope through this one rule so they
    /// agree at every offset. See [`cursor_scope()`] for the rule.
    pub(crate) fn cursor_scope(self, db: &dyn Db, offset: TextSize) -> ScopeId {
        cursor_scope(self.semantic_index(db), &self.parse(db).syntax(), offset)
    }
}

/// The scope a cursor at `offset` is in.
///
/// [`SemanticIndex::scope_at()`] hit-tests half-open scope ranges, which are
/// trimmed syntax ranges ending at a scope's last token. That suits an offset
/// on a token, but a cursor between tokens can sit at or past a range's
/// excluded end and still continue the scope: in `f <- function(x) x|` and
/// `f <- function(x) |` the cursor is in the function body, and so is a cursor
/// on the trailing lines of an unclosed `function(x) {` at the end of the file.
///
/// So when the last token before the cursor ends a scope, the cursor stays in
/// the innermost such scope that it continues:
///
/// - After a closing `}`, the cursor has left the braces and the scope they
///   end, unless it is still inside an unclosed delimiter of that scope.
/// - On the same line as any other token, the cursor continues the token's
///   expression.
/// - After a newline, the statement has ended unless it is unfinished: an
///   operand or body is missing (`x +`, `function(x)`), the cursor is inside
///   an unclosed delimiter of the scope, or inside parentheses or brackets
///   around it, where R does not end an expression at a newline.
///
/// `scope_at()` itself keeps half-open ranges because `definition_at()` and
/// `use_at()` hit-test tokens, and the rule above needs the syntax tree, which
/// the semantic index does not keep.
pub(crate) fn cursor_scope(index: &SemanticIndex, root: &RSyntaxNode, offset: TextSize) -> ScopeId {
    let offset = offset.min(root.text_range_with_trivia().end());
    let (base, _) = index.scope_at(offset);

    let Some(token) = last_token_before(root, offset) else {
        return base;
    };
    let token_range = token.text_trimmed_range();
    if token_range.end() > offset {
        // The cursor is inside the token, so the half-open test is exact.
        return base;
    }

    // The scopes ending at `token`, innermost first, are the ancestors of the
    // scope holding `token` strictly below `base`. When `base` is not among
    // those ancestors, the cursor is at the start of a scope `token` isn't in.
    let (inner, _) = index.scope_at(token_range.start());
    if !index.ancestor_scope_ids(inner).any(|id| id == base) {
        return base;
    }

    let gap = root
        .text_with_trivia()
        .slice(TextRange::new(token_range.end(), offset));
    let newline = gap.contains_char('\n') || gap.contains_char('\r');
    let closes_braces = token.kind() == RSyntaxKind::R_CURLY;
    let unfinished = misses_trailing_operand(&token);
    let open = innermost_open_delimiter(&token, offset);

    for scope_id in index.ancestor_scope_ids(inner) {
        if scope_id == base {
            break;
        }
        let scope_start = index.scope(scope_id).range().start();
        let inside_own_delimiter = open.as_ref().is_some_and(|open| open.start >= scope_start);
        let inside_parens = open
            .as_ref()
            .is_some_and(|open| open.kind == DelimiterKind::Parens);

        let continues = if closes_braces {
            inside_own_delimiter
        } else if !newline {
            true
        } else {
            unfinished || inside_own_delimiter || inside_parens
        };
        if continues {
            return scope_id;
        }
    }

    base
}

/// The last non-trivia token starting before `offset`.
fn last_token_before(root: &RSyntaxNode, offset: TextSize) -> Option<RSyntaxToken> {
    let mut token = match root.token_at_offset(offset) {
        TokenAtOffset::None => root.last_token()?,
        TokenAtOffset::Single(token) => token,
        TokenAtOffset::Between(left, _) => left,
    };
    // Skip tokens the cursor sits before or in the leading trivia of, and
    // empty tokens such as EOF.
    while token.text_trimmed_range().start() >= offset || token.text_trimmed().is_empty() {
        token = token.prev_token()?;
    }
    Some(token)
}

/// Whether a construct ending at `token` is missing its trailing operand or
/// body, so the expression continues on the next line.
fn misses_trailing_operand(token: &RSyntaxToken) -> bool {
    let end = token.text_trimmed_range().end();
    token
        .ancestors()
        .take_while(|node| node.text_trimmed_range().end() == end)
        .any(|node| match node.kind() {
            RSyntaxKind::R_BINARY_EXPRESSION => missing(&node, RBinaryExpression::right),
            RSyntaxKind::R_UNARY_EXPRESSION => missing(&node, RUnaryExpression::argument),
            RSyntaxKind::R_EXTRACT_EXPRESSION => missing(&node, RExtractExpression::right),
            RSyntaxKind::R_NAMESPACE_EXPRESSION => missing(&node, RNamespaceExpression::right),
            RSyntaxKind::R_FUNCTION_DEFINITION => missing(&node, RFunctionDefinition::body),
            RSyntaxKind::R_IF_STATEMENT => missing(&node, RIfStatement::consequence),
            RSyntaxKind::R_ELSE_CLAUSE => missing(&node, RElseClause::alternative),
            RSyntaxKind::R_FOR_STATEMENT => missing(&node, RForStatement::body),
            RSyntaxKind::R_WHILE_STATEMENT => missing(&node, RWhileStatement::body),
            RSyntaxKind::R_REPEAT_STATEMENT => missing(&node, RRepeatStatement::body),
            _ => false,
        })
}

fn missing<N: AstNode<Language = aether_syntax::RLanguage>, T>(
    node: &RSyntaxNode,
    child: impl Fn(&N) -> SyntaxResult<T>,
) -> bool {
    N::cast_ref(node).is_some_and(|node| child(&node).is_err())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DelimiterKind {
    Braces,
    /// Parentheses or brackets.
    Parens,
}

/// A delimited construct around the cursor whose closing token is missing or
/// comes at or after the cursor.
struct OpenDelimiter {
    start: TextSize,
    kind: DelimiterKind,
}

fn innermost_open_delimiter(token: &RSyntaxToken, offset: TextSize) -> Option<OpenDelimiter> {
    token.ancestors().find_map(|node| {
        let (kind, closing) = closing_delimiter(&node)?;
        let open = match closing {
            Ok(closing) => closing.text_trimmed_range().start() >= offset,
            Err(_) => true,
        };
        open.then(|| OpenDelimiter {
            start: node.text_trimmed_range().start(),
            kind,
        })
    })
}

fn closing_delimiter(node: &RSyntaxNode) -> Option<(DelimiterKind, SyntaxResult<RSyntaxToken>)> {
    let delimiter = match node.kind() {
        RSyntaxKind::R_BRACED_STATEMENTS => (
            DelimiterKind::Braces,
            RBracedStatements::cast_ref(node)?.r_curly_token(),
        ),
        RSyntaxKind::R_PARENTHESIZED_EXPRESSION => (
            DelimiterKind::Parens,
            RParenthesizedExpression::cast_ref(node)?.r_paren_token(),
        ),
        RSyntaxKind::R_CALL_ARGUMENTS => (
            DelimiterKind::Parens,
            RCallArguments::cast_ref(node)?.r_paren_token(),
        ),
        RSyntaxKind::R_SUBSET_ARGUMENTS => (
            DelimiterKind::Parens,
            RSubsetArguments::cast_ref(node)?.r_brack_token(),
        ),
        RSyntaxKind::R_SUBSET2_ARGUMENTS => (
            DelimiterKind::Parens,
            RSubset2Arguments::cast_ref(node)?.r_brack2_token(),
        ),
        RSyntaxKind::R_PARAMETERS => (
            DelimiterKind::Parens,
            RParameters::cast_ref(node)?.r_paren_token(),
        ),
        _ => return None,
    };
    Some(delimiter)
}
