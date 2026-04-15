use aether_syntax::AnyRSelector;
use aether_syntax::RNamespaceExpression;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use oak_index::semantic_index::SemanticIndex;
use oak_index::DefinitionId;
use oak_index::ScopeId;
use oak_index::UseId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    Definition {
        scope_id: ScopeId,
        def_id: DefinitionId,
    },

    Use {
        scope_id: ScopeId,
        use_id: UseId,
    },

    NamespaceAccess {
        package: String,
        symbol: String,
        internal: bool,
        package_range: TextRange,
        symbol_range: TextRange,
    },
}

impl Identifier {
    pub fn classify(root: &RSyntaxNode, index: &SemanticIndex, offset: TextSize) -> Option<Self> {
        if let Some((scope_id, def_id)) = index.definition_at_offset(offset) {
            return Some(Identifier::Definition { scope_id, def_id });
        }

        if let Some((scope_id, use_id)) = index.use_at_offset(offset) {
            return Some(Identifier::Use { scope_id, use_id });
        }

        classify_namespace(root, offset)
    }
}

fn classify_namespace(root: &RSyntaxNode, offset: TextSize) -> Option<Identifier> {
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
