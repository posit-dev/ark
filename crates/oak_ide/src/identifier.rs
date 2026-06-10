use aether_syntax::AnyRSelector;
use aether_syntax::RNamespaceExpression;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use aether_syntax::RSyntaxToken;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use biome_rowan::TokenAtOffset;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use oak_semantic::semantic_index::Definition;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::Use;
use oak_semantic::DefinitionId;
use oak_semantic::ScopeId;
use oak_semantic::UseId;

/// Semantic identity of identifier at cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier<'index> {
    Definition {
        scope_id: ScopeId,
        def_id: DefinitionId,
        def: &'index Definition,
        name: &'index str,
    },

    Use {
        scope_id: ScopeId,
        use_id: UseId,
        use_site: &'index Use,
        name: &'index str,
    },

    NamespaceAccess {
        package: String,
        symbol: String,
        internal: bool,
        package_range: TextRange,
        symbol_range: TextRange,
    },
}

impl<'index> Identifier<'index> {
    pub fn classify(
        index: &'index SemanticIndex,
        root: &RSyntaxNode,
        offset: TextSize,
    ) -> Option<Self> {
        let offset = snap_to_name_at_boundary(root, offset);
        Self::classify_at(index, root, offset)
    }

    fn classify_at(
        index: &'index SemanticIndex,
        root: &RSyntaxNode,
        offset: TextSize,
    ) -> Option<Self> {
        // `Import` definitions have empty ranges (no physical text position,
        // since `source()` injects them) so `definition_at()` skips them. If
        // the cursor is on the `source` symbol, the offset classifies instead
        // as a use of `source` via the check below.
        if let Some((scope_id, def_id, def)) = index.definition_at(offset) {
            let name = index.symbols(scope_id).symbol(def.symbol()).name();
            return Some(Identifier::Definition {
                scope_id,
                def_id,
                def,
                name,
            });
        }

        if let Some((scope_id, use_id, use_site)) = index.use_at(offset) {
            let name = index.symbols(scope_id).symbol(use_site.symbol()).name();
            return Some(Identifier::Use {
                scope_id,
                use_id,
                use_site,
                name,
            });
        }

        if let Some(namespace_access) = classify_namespace(root, offset) {
            return Some(namespace_access);
        }

        None
    }
}

fn classify_namespace<'index>(root: &RSyntaxNode, offset: TextSize) -> Option<Identifier<'index>> {
    let token = root.token_at_offset(offset).right_biased()?;

    let ns_expr = token
        .parent()
        .and_then(|p| p.ancestors().find_map(RNamespaceExpression::cast))?;

    let left = ns_expr.left().ok()?;
    let right = ns_expr.right().ok()?;

    let package = selector_name(&left)?;
    let symbol = selector_name(&right)?;

    let internal = ns_expr.operator().ok()?.kind() == RSyntaxKind::COLON3;
    let package_range = left.syntax().text_trimmed_range();
    let symbol_range = right.syntax().text_trimmed_range();

    Some(Identifier::NamespaceAccess {
        package,
        symbol,
        internal,
        package_range,
        symbol_range,
    })
}

fn selector_name(selector: &AnyRSelector) -> Option<String> {
    match selector {
        AnyRSelector::RIdentifier(ident) => Some(ident.name_text()),
        AnyRSelector::RStringValue(s) => s.string_text(),
        _ => None,
    }
}

/// At a token boundary, snap `offset` to the start of an adjacent name-like
/// token (regular identifier or string-form-def literal) if either neighbor
/// qualifies. Otherwise return `offset` unchanged.
///
/// LSP clients commonly land the cursor at the trailing edge of an identifier
/// (`offset == range.end`), e.g. after a double-click followed by a request. A
/// half-open `TextRange::contains` would miss this, so we snap the offset
/// towards a name-like token. Mirrors rust-analyzer's `pick_best_token` and
/// ty's `Tokens::at_offset` + token-kind ranking.
///
/// Two cases to handle:
/// - `Between(left, right)`: cursor sits exactly between two tokens; pick
///   the name-like neighbor.
/// - `Single(token)`: cursor lies within a token's full range but past its
///   trimmed range (common when the identifier has trailing trivia like a
///   newline). Snap to the trimmed start so `contains` matches.
fn snap_to_name_at_boundary(root: &RSyntaxNode, offset: TextSize) -> TextSize {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => offset,
        TokenAtOffset::Single(token) => {
            if is_name_token(&token) {
                token.text_trimmed_range().start()
            } else {
                offset
            }
        },
        TokenAtOffset::Between(left, right) => {
            if is_name_token(&left) {
                left.text_trimmed_range().start()
            } else if is_name_token(&right) {
                right.text_trimmed_range().start()
            } else {
                offset
            }
        },
    }
}

/// Tokens that participate in a name binding: regular identifiers and
/// string literals (which are valid def sites via `"foo" <- 1`).
fn is_name_token(token: &RSyntaxToken) -> bool {
    matches!(
        token.kind(),
        RSyntaxKind::IDENT | RSyntaxKind::R_STRING_LITERAL
    )
}
