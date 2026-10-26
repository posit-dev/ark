use anyhow::anyhow;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_core::identifier::RName;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Name;
use oak_db::NameSpelling;
use oak_db::RootKind;

use crate::find_references;
use crate::RenameEdit;

/// Identify the renamable identifier at `offset`, returning its range and
/// current (unquoted) name.
///
/// Returns `Ok(None)` when the cursor isn't on something we can rename (a
/// non-identifier, a `pkg::sym` namespace access, or a `$`/`@` member name),
/// so the client simply offers no rename. Returns `Err` when the cursor is on
/// a renamable identifier that we still refuse, a symbol defined in an
/// installed package or bound by a name we can't rewrite (`assign(c("x"), 1)`),
/// so the client can surface why at prepare time.
pub fn prepare_rename(
    db: &dyn Db,
    file: File,
    offset: TextSize,
) -> anyhow::Result<Option<(TextRange, String)>> {
    Ok(renamable_at(db, file, offset)?.map(|(range, name)| (range, name.text(db).to_string())))
}

/// Rename the symbol at `offset` to `new_name`, returning one edit per site
/// with the replacement text already rendered in that site's spelling.
///
/// Each site is rendered here rather than by the caller because how a name
/// appears is R-language semantics: a use is a bare identifier, but a
/// string-form binding (e.g. `assign("x", ..)`) must remain a quoted string.
///
/// Returns `Err` when the cursor isn't on a renamable identifier, when the
/// symbol resolves to a definition in an installed package (which we can't
/// edit), when `new_name` isn't a valid R name, when nothing in the database
/// binds the cursor's symbol, or when a site isn't a name we can rewrite in
/// place (a computed name or a raw string). With no binding a rename would
/// produce no edits, so we refuse rather than silently succeed.
pub fn rename(
    db: &dyn Db,
    file: File,
    offset: TextSize,
    new_name: &str,
) -> anyhow::Result<Vec<RenameEdit>> {
    let Some(_) = renamable_at(db, file, offset)? else {
        return Err(anyhow!("Can't rename identifier at cursor."));
    };

    let new_name = RName::parse(new_name)?;

    let sites = find_references(db, file, offset, true);
    if sites.is_empty() {
        return Err(anyhow!(
            "Can't rename: symbol has no binding in the workspace."
        ));
    }

    // Refuse the whole rename if any site can't be rewritten in place, as with
    // the computed name in `assign(c("x"), 1)`. Skipping that binding would
    // leave it under the old name while renaming its uses.
    sites
        .into_iter()
        .map(|site| {
            let delimiter = editable_delimiter(db, site.file, site.range)?;
            Ok(RenameEdit {
                file: site.file,
                range: site.range,
                new_text: render(delimiter, &new_name),
            })
        })
        .collect()
}

/// Preserve quoted sites because removing their quotes changes a binding name
/// into a variable reference.
fn render(delimiter: Option<char>, new_name: &RName) -> String {
    match delimiter {
        Some(delimiter) => new_name.quoted(delimiter),
        None => new_name.identifier().to_string(),
    }
}

/// The opening quote of the name at `range`, or `None` for an identifier.
/// Errors when the site isn't a name that `render()` can rewrite in place.
fn editable_delimiter(db: &dyn Db, file: File, range: TextRange) -> anyhow::Result<Option<char>> {
    let source = file.source_text(db);
    let text = &source[usize::from(range.start())..usize::from(range.end())];
    match file.name_spelling_at(db, range) {
        Some(NameSpelling::Identifier) => Ok(None),
        Some(NameSpelling::Quoted(delimiter)) => Ok(Some(delimiter)),
        // A new name could need different dashes or brackets to delimit it,
        // so raw strings are not rewritten.
        Some(NameSpelling::RawString) => Err(anyhow!(
            "Can't rename: symbol is bound by a raw string (`{text}`)."
        )),
        None => Err(anyhow!(
            "Can't rename: symbol is bound by a computed name (`{text}`)."
        )),
    }
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

    // Refuse at prepare time when a definition reaching the cursor can't be
    // rewritten, so the client doesn't ask for a new name in vain. `rename()`
    // still checks every site, including definitions this lookup doesn't
    // reach.
    for def in &defs {
        let Some(name_range) = def.name_range(db) else {
            continue;
        };
        editable_delimiter(db, def.file(db), name_range)?;
    }

    Ok(Some((range, name)))
}

fn is_library_def(db: &dyn Db, def: Definition) -> bool {
    def.file(db)
        .root(db)
        .is_some_and(|root| root.kind(db) == RootKind::Library)
}
