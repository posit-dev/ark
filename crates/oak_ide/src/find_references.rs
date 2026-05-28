use std::collections::HashSet;

use aether_syntax::RSyntaxNode;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::DefinitionId;
use oak_semantic::ScopeId;

use crate::FilePosition;
use crate::FileRange;
use crate::Identifier;

/// Result of [`find_references`].
///
/// TODO(salsa): the `locally_scoped` flag is temporary infrastructure. It
/// exists today because cross-file references are handled outside the semantic
/// index by a separate textual workspace scan, and the caller needs to know
/// whether to run that scan. When cross-file resolution lands,
/// [`find_references`] becomes a single salsa-aware query and the
/// locally-scoped optimization moves inside it as an internal short-circuit, at
/// which point this field disappears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct References {
    /// In-file occurrences of the target binding, in source order.
    pub ranges: Vec<FileRange>,
    /// Whether the target binding is entirely locally scoped. When `true`, no
    /// cross-file references are possible (function-local bindings aren't
    /// visible to other files) so callers can skip any workspace-wide candidate
    /// scan. When `false` the target is at file scope, the symbol is unbound,
    /// or the cursor doesn't resolve to a binding at all: all cases where
    /// cross-file matches are possible.
    pub locally_scoped: bool,
}

/// Find all in-file references to the symbol at offset.
///
/// The target is usually a single def. It grows when a use is reached by
/// conditional defs, or when a free variable picks up multiple visible
/// defs from an enclosing scope.
///
/// Returns empty ranges for:
/// - Non-identifier cursors (no `Identifier::classify` match).
/// - `pkg::sym` namespace access. TODO(salsa).
/// - Truly free variables. These are handled by the ark-layer cross-file
///   fallback, TODO(salsa).
///
/// TODO(salsa): switch the candidate pool to a textual scan so the same
/// `candidates -> refine via name resolution` path works for intra-file
/// and cross-file uniformly (r-a / ty's approach).
///
/// TODO(places): `foo$bar` / `foo@bar` member accesses aren't tracked by
/// the semantic index, so cursor on a member name returns empty here.
pub fn find_references(
    index: &SemanticIndex,
    root: &RSyntaxNode,
    position: &FilePosition,
    include_declaration: bool,
) -> References {
    let Some(ident) = Identifier::classify(index, root, position.offset) else {
        return References {
            ranges: Vec::new(),
            locally_scoped: false,
        };
    };

    // Compute the cursor's reaching defs. Same operation we'll run on
    // every candidate use below.
    let (target_defs, name): (HashSet<(ScopeId, DefinitionId)>, String) = match ident {
        Identifier::Definition {
            scope_id,
            def_id,
            name,
            ..
        } => (
            std::iter::once((scope_id, def_id)).collect(),
            name.to_string(),
        ),
        Identifier::Use {
            scope_id,
            use_id,
            name,
            ..
        } => (
            index.reaching_definitions(scope_id, use_id).collect(),
            name.to_string(),
        ),
        Identifier::NamespaceAccess { .. } => {
            return References {
                ranges: Vec::new(),
                locally_scoped: false,
            };
        },
    };

    if target_defs.is_empty() {
        return References {
            ranges: Vec::new(),
            locally_scoped: false,
        };
    }

    // All target defs in scopes other than the file scope means the
    // binding can only be referenced from within this file.
    let file_scope = ScopeId::from(0);
    let locally_scoped = target_defs.iter().all(|(scope, _)| *scope != file_scope);

    let mut results: Vec<FileRange> = Vec::new();

    // Definition sites come straight from `target_defs`.
    if include_declaration {
        for &(scope_id, def_id) in &target_defs {
            let def = &index.definitions(scope_id)[def_id];
            results.push(FileRange {
                file: position.file.clone(),
                range: def.range(),
            });
        }
    }

    // Walk all uses in every scope and check for each use of the same name
    // whether its binding set intersects the target.
    for scope_id in index.scope_ids() {
        let symbols = index.symbols(scope_id);
        let Some(symbol_id) = symbols.id(&name) else {
            // The scope doesn't have any uses for that symbol
            continue;
        };

        for (use_id, use_site) in index.uses(scope_id).iter() {
            if use_site.symbol() != symbol_id {
                continue;
            }
            let intersects = index
                .reaching_definitions(scope_id, use_id)
                .any(|d| target_defs.contains(&d));
            if !intersects {
                continue;
            }
            results.push(FileRange {
                file: position.file.clone(),
                range: use_site.range(),
            });
        }
    }

    // Defs are emitted in `target_defs` (HashSet) iteration order, which
    // is non-deterministic. Sort by start offset so callers see source
    // order regardless of how we collected results.
    //
    // TODO(salsa): once cross-file resolution lands, this becomes
    // file-then-offset: current file first, then other files in some
    // stable order (probably alphabetical by URL), with source order
    // preserved within each file.
    results.sort_by_key(|r| r.range.start());

    References {
        ranges: results,
        locally_scoped,
    }
}
