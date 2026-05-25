use aether_syntax::RSyntaxNode;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::ScopeId;
use oak_semantic::UseId;
use url::Url;

use crate::FileOffset;
use crate::Identifier;
use crate::NavigationTarget;

/// Resolve the symbol at `offset` in a file.
///
/// TODO(salsa) Within-file only. `pkg::sym` and any unresolved free variable
/// currently return an empty list. Cross-file resolution lives in the
/// salsa-backed path and will be wired in later.
pub fn goto_definition(
    index: &SemanticIndex,
    root: &RSyntaxNode,
    pos: &FileOffset,
) -> Vec<NavigationTarget> {
    let Some(ident) = Identifier::classify(index, root, pos.offset) else {
        return Vec::new();
    };

    match ident {
        Identifier::Definition { scope_id, def_id } => {
            let def = &index.definitions(scope_id)[def_id];
            let name = index.symbols(scope_id).symbol(def.symbol()).name();

            vec![NavigationTarget {
                file: pos.file.clone(),
                name: name.to_string(),
                full_range: def.range(),
                focus_range: def.range(),
            }]
        },
        Identifier::Use { scope_id, use_id } => resolve_use(index, &pos.file, scope_id, use_id),
        Identifier::NamespaceAccess { .. } => Vec::new(),
    }
}

fn resolve_use(
    index: &SemanticIndex,
    file: &Url,
    scope_id: ScopeId,
    use_id: UseId,
) -> Vec<NavigationTarget> {
    let symbol = index.uses(scope_id)[use_id].symbol();
    let symbol_name = index.symbols(scope_id).symbol(symbol).name().to_string();

    // `reaching_definitions` unions the local use-def map with the
    // enclosing-scope snapshot when `may_be_unbound` is true. That
    // covers the conditional-local-plus-outer-binding case where both
    // the inner conditional def and the outer def can reach the use.
    //
    // TODO(salsa): when the result is empty, fall through to cross-file
    // resolution (file imports / package exports).
    index
        .reaching_definitions(scope_id, use_id)
        .map(|(scope, def_id)| {
            let def = &index.definitions(scope)[def_id];
            NavigationTarget {
                file: file.clone(),
                name: symbol_name.clone(),
                full_range: def.range(),
                focus_range: def.range(),
            }
        })
        .collect()
}
