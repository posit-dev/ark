use biome_rowan::TextSize;
use oak_index::external::resolve_external_name;
use oak_index::external::BindingSource;
use oak_index::external::ExternalDefinition;
use oak_index::semantic_index::SemanticIndex;
use oak_package::library::Library;
use url::Url;

use crate::NavigationTarget;

/// Resolve the symbol at `offset` in a file.
///
/// First tries local resolution via the use-def map. If the symbol may be
/// unbound locally (no definitions on all control-flow paths), falls through
/// to external resolution using the provided scope chain.
///
/// Returns an empty `Vec` if the offset doesn't point at a use site, or if
/// the symbol cannot be resolved locally or externally.
pub fn goto_definition(
    file: &Url,
    index: &SemanticIndex,
    scope_chain: &[BindingSource],
    library: &Library,
    offset: TextSize,
) -> Vec<NavigationTarget> {
    let Some((scope_id, use_id)) = index.use_at_offset(offset) else {
        return Vec::new();
    };

    let use_def_map = index.use_def_map(scope_id);
    let bindings = use_def_map.bindings_at_use(use_id);

    let use_site = &index.uses(scope_id)[use_id];
    let symbol_name = index.symbols(scope_id).symbol(use_site.symbol()).name();

    // If we have local definitions, return them. Even when `may_be_unbound`
    // (conditional defs), the user most likely wants the local binding.
    let definitions = bindings.definitions();
    if !definitions.is_empty() {
        return definitions
            .iter()
            .map(|&def_id| {
                let def = &index.definitions(scope_id)[def_id];
                NavigationTarget {
                    file: file.clone(),
                    name: symbol_name.to_string(),
                    full_range: def.range(),
                    focus_range: def.range(),
                }
            })
            .collect();
    }

    // No local definitions. If we're in a nested scope, check enclosing
    // bindings (the symbol might be defined in an outer function scope).
    if let Some((enclosing_scope, enclosing_bindings)) = index.enclosing_bindings(scope_id, use_id)
    {
        let enclosing_defs = enclosing_bindings.definitions();
        if !enclosing_defs.is_empty() {
            return enclosing_defs
                .iter()
                .map(|&def_id| {
                    let def = &index.definitions(enclosing_scope)[def_id];
                    NavigationTarget {
                        file: file.clone(),
                        name: symbol_name.to_string(),
                        full_range: def.range(),
                        focus_range: def.range(),
                    }
                })
                .collect();
        }
    }

    // No local or enclosing definitions. Try external resolution.
    let Some(external) = resolve_external_name(library, scope_chain, symbol_name) else {
        return Vec::new();
    };

    match external {
        ExternalDefinition::ProjectFile { file, name, range } => {
            vec![NavigationTarget {
                file,
                name,
                full_range: range,
                focus_range: range,
            }]
        },
        // No file/range to navigate to for package symbols (yet).
        ExternalDefinition::Package { .. } => Vec::new(),
    }
}
