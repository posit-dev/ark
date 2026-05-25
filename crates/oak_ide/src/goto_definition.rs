use aether_syntax::RSyntaxNode;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::Use;
use oak_semantic::DefinitionId;
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
        Identifier::Use { scope_id, use_id } => {
            let use_site = &index.uses(scope_id)[use_id];
            resolve_use(index, &pos.file, scope_id, use_id, use_site)
        },
        Identifier::NamespaceAccess { .. } => Vec::new(),
    }
}

fn resolve_use(
    index: &SemanticIndex,
    file: &Url,
    scope_id: ScopeId,
    use_id: UseId,
    use_site: &Use,
) -> Vec<NavigationTarget> {
    let symbol_name = index.symbols(scope_id).symbol(use_site.symbol()).name();

    let local_targets = |scope, defs: &[DefinitionId]| -> Vec<NavigationTarget> {
        defs.iter()
            .map(|&def_id| {
                let def = &index.definitions(scope)[def_id];
                NavigationTarget {
                    file: file.clone(),
                    name: symbol_name.to_string(),
                    full_range: def.range(),
                    focus_range: def.range(),
                }
            })
            .collect()
    };

    let bindings = index.use_def_map(scope_id).bindings_at_use(use_id);
    let definitions = bindings.definitions();
    if !definitions.is_empty() {
        return local_targets(scope_id, definitions);
    }

    // Free in this scope: walk enclosing scopes (the symbol might be bound
    // in an outer function or at file scope).
    if let Some((enclosing_scope, enclosing_bindings)) =
        index.enclosing_bindings(scope_id, use_site.symbol())
    {
        let enclosing_defs = enclosing_bindings.definitions();
        if !enclosing_defs.is_empty() {
            return local_targets(enclosing_scope, enclosing_defs);
        }
    }

    // TODO(salsa): Use salsa-based resolution of file imports for external targets

    Vec::new()
}
