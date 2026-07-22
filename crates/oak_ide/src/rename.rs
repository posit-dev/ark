use anyhow::anyhow;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_core::identifier::quote_name;
use oak_core::identifier::to_identifier_text;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Name;
use oak_db::RootKind;

use crate::find_references;
use crate::FileRange;
use crate::RenameEdit;

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

/// Rename the symbol at `offset` to `new_name`, returning one edit per site
/// with the replacement text already rendered in that site's spelling.
///
/// Each site is rendered here rather than by the caller because how a name
/// appears is R-language semantics: a use is a bare identifier, but a
/// string-form binding (e.g. `assign("x", ..)`) must remain a quoted string.
///
/// Returns `Err` when the cursor isn't on a renamable identifier, when the
/// symbol resolves to a definition in an installed package (which we can't
/// edit), when `new_name` isn't a valid R name, or when nothing in the database
/// binds the cursor's symbol. In that last case a rename would produce no
/// edits, so we refuse rather than silently succeed.
pub fn rename(
    db: &dyn Db,
    file: File,
    offset: TextSize,
    new_name: &str,
) -> anyhow::Result<Vec<RenameEdit>> {
    let Some(_) = renamable_at(db, file, offset)? else {
        return Err(anyhow!("Can't rename identifier at cursor."));
    };

    // Normalize to canonical R identifier syntax (backtick-wrapped if needed) up
    // front, so an invalid name fails before we produce any edits, uniformly for
    // every symbol regardless of how its sites are spelled.
    let identifier_text = to_identifier_text(new_name)?;

    let sites = find_references(db, file, offset, true);
    if sites.is_empty() {
        return Err(anyhow!(
            "Can't rename: symbol has no binding in the workspace."
        ));
    }

    let edits = sites
        .into_iter()
        .map(|site| {
            let new_text = render_at_site(db, &site, new_name, &identifier_text);
            RenameEdit {
                file: site.file,
                range: site.range,
                new_text,
            }
        })
        .collect();
    Ok(edits)
}

/// Render the replacement text for one site. A site spelled as a quoted string
/// (`assign("x", ..)`, `"x" <- ..`) stays a string, keeping its delimiter,
/// because dropping the quotes would turn the name into a variable reference and
/// change the program. Every other site is a bare identifier, rendered as the
/// already-validated `identifier_text`.
fn render_at_site(db: &dyn Db, site: &FileRange, new_name: &str, identifier_text: &str) -> String {
    let source = site.file.source_text(db);
    match string_delimiter(&source[..], site.range) {
        Some(delimiter) => quote_name(new_name, delimiter),
        None => identifier_text.to_string(),
    }
}

/// The opening quote of the string literal at `range` in `source`, or `None`
/// when the site is a bare identifier.
fn string_delimiter(source: &str, range: TextRange) -> Option<char> {
    let slice = &source[usize::from(range.start())..usize::from(range.end())];
    match slice.chars().next() {
        Some(delimiter @ ('"' | '\'')) => Some(delimiter),
        _ => None,
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

    Ok(Some((range, name)))
}

fn is_library_def(db: &dyn Db, def: Definition) -> bool {
    def.file(db)
        .root(db)
        .is_some_and(|root| root.kind(db) == RootKind::Library)
}
