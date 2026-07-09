use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Identifier;
use oak_db::Name;
use oak_db::NamespaceVisibility;

use crate::NavigationTarget;

/// Resolve the name at `offset` in `file` and describe where to jump.
///
/// Thin wrapper over [`File::resolve_at`], which runs R's whole lookup chain
/// (lexical scopes, function-body resolution, top-level reaching defs, file
/// imports). Each binding that could reach the use becomes a
/// [`NavigationTarget`] in its own file's coordinates, so an ambiguous name
/// (e.g. defined on both arms of an `if`/`else`) yields several. Empty means
/// the name isn't reachable, rather than guessing by name across the workspace.
///
/// `classify` snaps the cursor to the name token first, so a cursor resting on
/// the trailing edge of an identifier still resolves. `range.start()` is the
/// snapped offset `resolve_at` expects. A member name (`$`/`@` RHS) has no
/// binding to jump to, so it yields nothing.
pub fn goto_definition(db: &dyn Db, file: File, offset: TextSize) -> Vec<NavigationTarget> {
    let defs = match Identifier::classify(db, file, offset) {
        Some(Identifier::Variable { range, .. }) => goto_local_variable(db, file, range),
        Some(Identifier::NamespaceAccess {
            package,
            name,
            visibility,
        }) => goto_namespace_access(db, package, name, visibility),
        Some(Identifier::Member { .. }) => return Vec::new(),
        None => return Vec::new(),
    };

    defs.into_iter()
        .filter_map(|def| navigation_target(db, def))
        .collect()
}

fn goto_local_variable(db: &dyn Db, file: File, range: TextRange) -> Vec<Definition<'_>> {
    file.resolve_at(db, range.start())
}

fn goto_namespace_access<'a>(
    db: &'a dyn Db,
    package: Name<'a>,
    name: Name<'a>,
    visibility: NamespaceVisibility,
) -> Vec<Definition<'a>> {
    db.package_by_name(package.text(db).as_str())
        .map(|package| package.resolve(db, name, visibility))
        .unwrap_or_default()
}

fn navigation_target(db: &dyn Db, def: Definition) -> Option<NavigationTarget> {
    let range = def.name_range(db)?;
    Some(NavigationTarget {
        file: def.file(db),
        name: def.name(db).text(db).to_string(),
        full_range: range,
        focus_range: range,
    })
}
