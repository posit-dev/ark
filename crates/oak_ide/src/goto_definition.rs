use biome_rowan::TextSize;
use oak_db::Db;
use oak_db::File;

use crate::NavigationTarget;

/// Resolve the name at `offset` in `file` and describe where to jump.
///
/// Thin wrapper over [`File::resolve_at`], which runs R's whole lookup chain
/// (lexical scopes, function-body resolution, top-level reaching defs, file
/// imports). Each binding that could reach the use becomes a
/// [`NavigationTarget`] in its own file's coordinates, so an ambiguous name
/// (e.g. defined on both arms of an `if`/`else`) yields several. Empty means
/// the name isn't reachable, rather than guessing by name across the workspace.
pub fn goto_definition(db: &dyn Db, file: File, offset: TextSize) -> Vec<NavigationTarget> {
    file.resolve_at(db, offset)
        .into_iter()
        .filter_map(|def| {
            let range = def.name_range(db)?;
            Some(NavigationTarget {
                file: def.file(db),
                name: def.name(db).text(db).to_string(),
                full_range: range,
                focus_range: range,
            })
        })
        .collect()
}
