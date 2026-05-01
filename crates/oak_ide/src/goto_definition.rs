use aether_syntax::RSyntaxNode;
use biome_rowan::TextSize;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::Use;
use oak_index::DefinitionId;
use oak_index::ScopeId;
use oak_index::UseId;
use oak_index::external::resolve_external_name;
use oak_index::external::resolve_in_package;
use oak_index::library::Library;
use oak_index::package_definitions::PackageDefinitionVisibility;
use oak_index::scope_layer::ScopeLayer;
use url::Url;

use crate::ExternalScope;
use crate::Identifier;
use crate::NavigationTarget;

/// Resolve the symbol at `offset` in a file.
///
/// Uses `Identifier::classify` to determine what the offset points at:
///
/// - Definition site (LHS of assignment, parameter, for variable):
///   navigates to itself. We round-trip through the index to get
///   `full_range` and `focus_range` from the `Definition`. Currently both
///   are the name range, but once we distinguish them `full_range` will
///   come from the `DefinitionKind`'s `RSyntaxNode` (the whole assignment /
///   parameter expression).
///
/// - Use site (name reference): resolves via the use-def map, enclosing
///   scopes, and the external scope chain.
///
/// - Namespace access (`pkg::sym` or `pkg:::sym`): resolves the symbol
///   directly in the named package.
///
/// Returns an empty `Vec` if the offset doesn't point at a known
/// identifier, or if the symbol cannot be resolved.
pub fn goto_definition(
    offset: TextSize,
    file: &Url,
    root: &RSyntaxNode,
    index: &SemanticIndex,
    scope: &ExternalScope,
    library: &Library,
) -> Vec<NavigationTarget> {
    let Some(ident) = Identifier::classify(root, index, offset) else {
        return Vec::new();
    };

    match ident {
        Identifier::Definition { scope_id, def_id } => {
            let def = &index.definitions(scope_id)[def_id];
            let name = index.symbols(scope_id).symbol(def.symbol()).name();

            vec![NavigationTarget {
                file: file.clone(),
                name: name.to_string(),
                full_range: def.range(),
                focus_range: def.range(),
            }]
        },
        Identifier::Use { scope_id, use_id } => {
            let use_site = &index.uses(scope_id)[use_id];
            resolve_use(
                index, scope_id, use_id, use_site, file, offset, scope, library,
            )
        },
        Identifier::NamespaceAccess {
            ref package,
            ref symbol,
            internal,
            ..
        } => {
            let visibility = if internal {
                PackageDefinitionVisibility::Internal
            } else {
                PackageDefinitionVisibility::Exported
            };
            resolve_namespace_access(symbol, package, visibility, library)
        },
    }
}

fn resolve_use(
    index: &SemanticIndex,
    scope_id: ScopeId,
    use_id: UseId,
    use_site: &Use,
    file: &Url,
    offset: TextSize,
    scope: &ExternalScope,
    library: &Library,
) -> Vec<NavigationTarget> {
    let use_def_map = index.use_def_map(scope_id);
    let bindings = use_def_map.bindings_at_use(use_id);

    let symbol_name = index.symbols(scope_id).symbol(use_site.symbol()).name();

    let definitions = bindings.definitions();

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

    let external_targets = || {
        let scope_chain = scope.at(index, offset);
        resolve_external(symbol_name, &scope_chain, library)
    };

    if !definitions.is_empty() {
        let mut targets = local_targets(scope_id, definitions);
        if bindings.may_be_unbound() {
            targets.extend(external_targets());
        }
        return targets;
    }

    // No local definitions. If we're in a nested scope, check enclosing
    // bindings (the symbol might be defined in an outer function scope).
    if let Some((enclosing_scope, enclosing_bindings)) =
        index.enclosing_bindings(scope_id, use_site.symbol())
    {
        let enclosing_defs = enclosing_bindings.definitions();
        if !enclosing_defs.is_empty() {
            let mut targets = local_targets(enclosing_scope, enclosing_defs);
            if enclosing_bindings.may_be_unbound() {
                targets.extend(external_targets());
            }
            return targets;
        }
    }

    external_targets()
}

fn resolve_namespace_access(
    symbol: &str,
    package: &str,
    visibility: PackageDefinitionVisibility,
    library: &Library,
) -> Vec<NavigationTarget> {
    let Some(external) = resolve_in_package(symbol, package, visibility, library) else {
        return Vec::new();
    };
    vec![external.into()]
}

fn resolve_external(
    symbol: &str,
    scope_chain: &[ScopeLayer],
    library: &Library,
) -> Vec<NavigationTarget> {
    let Some(external) = resolve_external_name(library, scope_chain, symbol) else {
        return Vec::new();
    };
    vec![external.into()]
}
