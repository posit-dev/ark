use aether_syntax::RSyntaxNode;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::ScopeId;
use oak_semantic::UseId;
use url::Url;

use crate::FilePosition;
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
    position: &FilePosition,
) -> Vec<NavigationTarget> {
    let Some(ident) = Identifier::classify(index, root, position.offset) else {
        return Vec::new();
    };

    match ident {
        Identifier::Definition { def, name, .. } => {
            vec![NavigationTarget {
                file: position.file.clone(),
                name: name.to_string(),
                full_range: def.range(),
                focus_range: def.range(),
            }]
        },
        Identifier::Use {
            scope_id,
            use_id,
            name,
            ..
        } => resolve_use(index, &position.file, scope_id, use_id, name),
        Identifier::NamespaceAccess { .. } => Vec::new(),
    }
}

fn resolve_use(
    index: &SemanticIndex,
    file: &Url,
    scope_id: ScopeId,
    use_id: UseId,
    name: &str,
) -> Vec<NavigationTarget> {
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
                name: name.to_string(),
                full_range: def.range(),
                focus_range: def.range(),
            }
        })
        .collect()
}
