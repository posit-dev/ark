use anyhow::anyhow;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_core::identifier::to_identifier_text;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Identifier;
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
/// current (unquoted) name. Returns `None` when the cursor is on a
/// non-identifier, a `pkg::sym` namespace access, or a `$`/`@` member name.
pub fn prepare_rename(db: &dyn Db, file: File, offset: TextSize) -> Option<(TextRange, String)> {
    match Identifier::classify(db, file, offset)? {
        Identifier::Variable { name, range } => Some((range, name.text(db).to_string())),
        Identifier::Member { .. } => None,
    }
}

/// Compute all rename edits across the database.
///
/// Returns `Err` when:
/// - `new_name` is empty, is an R reserved word, or contains a literal
///   backtick (which can't appear in a backtick-quoted identifier).
/// - The cursor isn't on a renamable identifier (no `prepare_rename()` target).
/// - Any target definition lives in an installed package.
/// - Nothing in the database binds the cursor's symbol. Rename would
///   produce no edits, so we refuse rather than silently succeed.
pub fn rename(
    db: &dyn Db,
    file: File,
    offset: TextSize,
    new_name: &str,
) -> anyhow::Result<RenameTargets> {
    let new_text = to_identifier_text(new_name)?;

    let ident = Identifier::classify(db, file, offset);
    let Some(Identifier::Variable { range, .. }) = &ident else {
        return Err(anyhow!("Can't rename identifier at cursor."));
    };

    for def in file.resolve_at(db, range.start()) {
        if is_library_def(db, def) {
            return Err(anyhow!(
                "Can't rename: symbol is defined in an installed package."
            ));
        }
    }

    let ranges = find_references(db, file, offset, true);
    if ranges.is_empty() {
        return Err(anyhow!(
            "Can't rename: symbol has no binding in the workspace."
        ));
    }

    Ok(RenameTargets { ranges, new_text })
}

fn is_library_def(db: &dyn Db, def: Definition) -> bool {
    def.file(db)
        .root(db)
        .is_some_and(|root| root.kind(db) == RootKind::Library)
}
