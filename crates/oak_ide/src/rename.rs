use anyhow::anyhow;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_core::identifier::to_identifier_text;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Identifier;
use oak_db::Name;
use oak_db::RootKind;

use crate::find_references;
use crate::FileRange;

/// All edits needed to rename the symbol at the cursor. Each range gets
/// replaced by `new_text`, across all files in the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameTargets {
    pub ranges: Vec<FileRange>,
    /// The canonical syntactic form of the new name (backtick-wrapped if
    /// needed). Same for every range in `ranges`.
    pub new_text: String,
}

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

/// Compute all rename edits across the database.
///
/// Returns `Err` when:
/// - `new_name` is empty, is an R reserved word, or contains a literal
///   backtick (which can't appear in a backtick-quoted identifier).
/// - The cursor isn't on a renamable identifier, or it resolves to an
///   installed package (both via `renamable_at`).
/// - Nothing in the database binds the cursor's symbol. Rename would
///   produce no edits, so we refuse rather than silently succeed.
pub fn rename(
    db: &dyn Db,
    file: File,
    offset: TextSize,
    new_name: &str,
) -> anyhow::Result<RenameTargets> {
    let new_text = to_identifier_text(new_name)?;

    let Some(_) = renamable_at(db, file, offset)? else {
        return Err(anyhow!("Can't rename identifier at cursor."));
    };

    let ranges = find_references(db, file, offset, true);
    if ranges.is_empty() {
        return Err(anyhow!(
            "Can't rename: symbol has no binding in the workspace."
        ));
    }

    Ok(RenameTargets { ranges, new_text })
}

fn renamable_at<'db>(
    db: &'db dyn Db,
    file: File,
    offset: TextSize,
) -> anyhow::Result<Option<(TextRange, Name<'db>)>> {
    let Some(Identifier::Variable { name, range }) = Identifier::classify(db, file, offset) else {
        return Ok(None);
    };

    for def in file.resolve_at(db, range.start()) {
        if is_library_def(db, def) {
            return Err(anyhow!(
                "Can't rename: symbol is defined in an installed package."
            ));
        }
    }

    Ok(Some((range, name)))
}

fn is_library_def(db: &dyn Db, def: Definition) -> bool {
    def.file(db)
        .root(db)
        .is_some_and(|root| root.kind(db) == RootKind::Library)
}
