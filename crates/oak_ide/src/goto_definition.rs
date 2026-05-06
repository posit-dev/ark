use std::collections::HashSet;

use aether_syntax::RSyntaxNode;
use biome_rowan::TextSize;
use oak_db::Db;
use oak_index::external::resolve_external_name;
use oak_index::external::resolve_in_package;
use oak_index::package_definitions::PackageDefinitionVisibility;
use oak_index::scope_layer::ScopeLayer;
use oak_index::semantic_index::DefinitionKind;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::Use;
use oak_index::DefinitionId;
use oak_index::ScopeId;
use oak_index::UseId;
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
    db: &dyn Db,
    offset: TextSize,
    file: &Url,
    root: &RSyntaxNode,
    index: &SemanticIndex,
    scope: &ExternalScope,
) -> Vec<NavigationTarget> {
    let Some(ident) = Identifier::classify(root, index, offset) else {
        return Vec::new();
    };

    match ident {
        Identifier::Definition { scope_id, def_id } => {
            let def = &index.definitions(scope_id)[def_id];
            let name = index.symbols(scope_id).symbol_id(def.symbol()).name();

            vec![NavigationTarget {
                file: file.clone(),
                name: name.to_string(),
                full_range: def.range(),
                focus_range: def.range(),
            }]
        },
        Identifier::Use { scope_id, use_id } => {
            let use_site = &index.uses(scope_id)[use_id];
            resolve_use(db, index, scope_id, use_id, use_site, offset, scope)
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
            resolve_namespace_access(db, symbol, package, visibility)
        },
    }
}

fn resolve_use(
    db: &dyn Db,
    index: &SemanticIndex,
    scope_id: ScopeId,
    use_id: UseId,
    use_site: &Use,
    offset: TextSize,
    scope: &ExternalScope,
) -> Vec<NavigationTarget> {
    let use_def_map = index.use_def_map(scope_id);
    let bindings = use_def_map.bindings_at_use(use_id);

    let symbol_name = index.symbols(scope_id).symbol_id(use_site.symbol()).name();

    let definitions = bindings.definitions();

    let local_targets = |scope, defs: &[DefinitionId]| -> Vec<NavigationTarget> {
        defs.iter()
            .filter_map(|&def_id| {
                let def = &index.definitions(scope)[def_id];
                match def.kind() {
                    DefinitionKind::Import { file, name, .. } => resolve_import(db, file, name),
                    _ => Some(NavigationTarget {
                        file: def.file().clone(),
                        name: symbol_name.to_string(),
                        full_range: def.range(),
                        focus_range: def.range(),
                    }),
                }
            })
            .collect()
    };

    let external_targets = || {
        let scope_chain = scope.at(index, offset);
        resolve_external(db, symbol_name, &scope_chain)
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

/// Chase a `DefinitionKind::Import` forwarding binding to the actual
/// definition in the target file. Recurses through chains of Import
/// definitions (e.g., a.R sources b.R sources c.R).
///
/// TODO(salsa): Move to `oak_index` once it depends on `oak_db`.
fn resolve_import(db: &dyn Db, file: &Url, name: &str) -> Option<NavigationTarget> {
    let mut visited = HashSet::new();
    resolve_import_inner(db, file, name, &mut visited)
}

fn resolve_import_inner(
    db: &dyn Db,
    file: &Url,
    name: &str,
    visited: &mut HashSet<(Url, String)>,
) -> Option<NavigationTarget> {
    if !visited.insert((file.clone(), name.to_string())) {
        return None;
    }

    let target_index = db.semantic_index(file)?;

    // Imports always target top-level definitions in the sourced file.
    let file_scope = ScopeId::from(0);
    let symbols = &target_index.symbols(file_scope);

    let (_def_id, def) = target_index
        .definitions(file_scope)
        .iter()
        .filter(|(_id, def)| symbols.symbol_id(def.symbol()).name() == name)
        .last()?;

    match def.kind() {
        DefinitionKind::Import {
            file: next_file,
            name: next_name,
            ..
        } => resolve_import_inner(db, next_file, next_name, visited),
        _ => Some(NavigationTarget {
            file: file.clone(),
            name: name.to_string(),
            full_range: def.range(),
            focus_range: def.range(),
        }),
    }
}

fn resolve_namespace_access(
    db: &dyn Db,
    symbol: &str,
    package: &str,
    visibility: PackageDefinitionVisibility,
) -> Vec<NavigationTarget> {
    let Some(external) = resolve_in_package(db.library(), package, symbol, visibility) else {
        return Vec::new();
    };
    vec![external.into()]
}

fn resolve_external(
    db: &dyn Db,
    symbol: &str,
    scope_chain: &[ScopeLayer],
) -> Vec<NavigationTarget> {
    let Some(external) = resolve_external_name(db.library(), scope_chain, symbol) else {
        return Vec::new();
    };
    vec![external.into()]
}
