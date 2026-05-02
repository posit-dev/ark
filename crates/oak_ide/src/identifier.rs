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
        let (scope_id, _) = index.scope_at(offset);

        if let Some((def_id, def)) = index.definitions(scope_id).contains(offset) {
            if def.file().is_none() {
                return Some(Identifier::Definition { scope_id, def_id });
            }
        }

        if let Some((use_id, _)) = index.uses(scope_id).contains(offset) {
            return Some(Identifier::Use { scope_id, use_id });
        }

        if let Some(namespace_access) = classify_namespace(root, offset) {
            return Some(namespace_access);
        }

        None
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
