use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_index::external::resolve_external_name;
use oak_index::external::BindingSource;
use oak_index::external::ExternalDefinition;
use oak_index::semantic_index::SemanticIndex;
use oak_package::library::Library;
use url::Url;

/// The result of resolving a symbol at a given offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDefinition {
    /// Defined locally in the same file.
    Local {
        /// Range of the definition site (the name being bound).
        range: TextRange,
    },

    /// Defined in another project file.
    ProjectFile {
        file: Url,
        name: String,
        range: TextRange,
    },

    /// Defined in an installed package.
    Package { package: String, name: String },
}

/// Resolve the symbol at `offset` in a file.
///
/// First tries local resolution via the use-def map. If the symbol may be
/// unbound locally (no definitions on all control-flow paths), falls through
/// to external resolution using the provided scope chain.
///
/// Returns `None` if the offset doesn't point at a use site, or if the
/// symbol cannot be resolved locally or externally.
pub fn goto_definition(
    index: &SemanticIndex,
    scope_chain: &[BindingSource],
    library: &Library,
    offset: TextSize,
) -> Option<ResolvedDefinition> {
    let (scope_id, use_id) = index.use_at_offset(offset)?;

    let use_def_map = index.use_def_map(scope_id);
    let bindings = use_def_map.bindings_at_use(use_id);

    // If we have local definitions, return the first one. Multiple
    // definitions arise from conditional assignments; we pick the first for
    // goto-definition. Even when `may_be_unbound` (conditional defs), the
    // user most likely wants the local binding.
    let definitions = bindings.definitions();
    if !definitions.is_empty() {
        let def_id = definitions[0];
        let def = &index.definitions(scope_id)[def_id];
        return Some(ResolvedDefinition::Local { range: def.range() });
    }

    // No local definitions. If we're in a nested scope, check enclosing
    // bindings (the symbol might be defined in an outer function scope).
    if let Some((enclosing_scope, enclosing_bindings)) = index.enclosing_bindings(scope_id, use_id)
    {
        let enclosing_defs = enclosing_bindings.definitions();
        if !enclosing_defs.is_empty() {
            let def_id = enclosing_defs[0];
            let def = &index.definitions(enclosing_scope)[def_id];
            return Some(ResolvedDefinition::Local { range: def.range() });
        }
    }

    // No local or enclosing definitions. Try external resolution.
    let use_site = &index.uses(scope_id)[use_id];
    let symbol_name = index.symbols(scope_id).symbol(use_site.symbol()).name();

    let external = resolve_external_name(library, scope_chain, symbol_name)?;

    match external {
        ExternalDefinition::ProjectFile { file, name, range } => {
            Some(ResolvedDefinition::ProjectFile { file, name, range })
        },
        ExternalDefinition::Package { package, name } => {
            Some(ResolvedDefinition::Package { package, name })
        },
    }
}
