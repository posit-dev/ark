use anyhow::anyhow;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Name;
use oak_db::RootKind;

use crate::find_references;
use crate::FileRange;

/// Identify the renamable identifier at `offset`, returning its range and
/// current (unquoted) name.
///
/// Returns `Ok(None)` when the cursor isn't on something we can rename (a
/// non-identifier, a `pkg::sym` namespace access, or a `$`/`@` member name),
/// so the client simply offers no rename. Returns `Err` when the cursor is on
/// a renamable identifier that we still refuse, today only a symbol defined in
/// an installed package, so the client can surface why at prepare time.
pub fn prepare_rename(
    db: &dyn Db,
    file: File,
    offset: TextSize,
) -> anyhow::Result<Option<(TextRange, String)>> {
    Ok(renamable_at(db, file, offset)?.map(|(range, name)| (range, name.text(db).to_string())))
}

/// Find every site to rename for the symbol at `offset`. The caller turns
/// these ranges into edits.
///
/// Returns `Err` when the cursor isn't on a renamable identifier or it
/// resolves to an installed package (both via `renamable_at`), or when nothing
/// in the database binds the cursor's symbol. In that last case a rename would
/// produce no edits, so we refuse rather than silently succeed.
pub fn rename(db: &dyn Db, file: File, offset: TextSize) -> anyhow::Result<Vec<FileRange>> {
    let Some(_) = renamable_at(db, file, offset)? else {
        return Err(anyhow!("Can't rename identifier at cursor."));
    };

    let ranges = find_references(db, file, offset, true);
    if ranges.is_empty() {
        return Err(anyhow!(
            "Can't rename: symbol has no binding in the workspace."
        ));
    }

    Ok(ranges)
}

fn renamable_at<'db>(
    db: &'db dyn Db,
    file: File,
    offset: TextSize,
) -> anyhow::Result<Option<(TextRange, Name<'db>)>> {
    let Some((name, range, defs)) = file.resolve_variable_at(db, offset) else {
        return Ok(None);
    };

    if defs.iter().any(|&def| is_library_def(db, def)) {
        return Err(anyhow!(
            "Can't rename: symbol is defined in an installed package."
        ));
    }

    Ok(Some((range, name)))
}

fn is_library_def(db: &dyn Db, def: Definition) -> bool {
    def.file(db)
        .root(db)
        .is_some_and(|root| root.kind(db) == RootKind::Library)
}
