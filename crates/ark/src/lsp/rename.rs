use std::collections::HashMap;

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use anyhow::Context;
use biome_rowan::TextRange;
use oak_core::identifier::to_identifier_text;
use oak_db::Db;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::PrepareRenameResponse;
use tower_lsp_server::ls_types::RenameParams;
use tower_lsp_server::ls_types::TextDocumentPositionParams;
use tower_lsp_server::ls_types::TextEdit;
use tower_lsp_server::ls_types::WorkspaceEdit;

use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

pub(crate) fn prepare_rename(
    params: TextDocumentPositionParams,
    state: &WorldState,
) -> anyhow::Result<Option<PrepareRenameResponse>> {
    let path = params.text_document.uri.to_document_path()?;
    let position = params.position;

    let db = &state.db;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_path(&path) else {
        return Ok(None);
    };

    let offset = from_proto::offset_from_position(position, file.line_index(db), encoding)?;

    let Some((range, placeholder)) = oak_ide::prepare_rename(db, file, offset)? else {
        return Ok(None);
    };

    let range = to_proto::range(range, file.line_index(db), encoding)?;
    Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
        range,
        placeholder,
    }))
}

pub(crate) fn rename(
    params: RenameParams,
    state: &WorldState,
) -> anyhow::Result<Option<WorkspaceEdit>> {
    let path = params
        .text_document_position
        .text_document
        .uri
        .to_document_path()?;
    let position = params.text_document_position.position;
    let new_name = params.new_name;

    let db = &state.db;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_path(&path) else {
        return Ok(None);
    };

    let offset = from_proto::offset_from_position(position, file.line_index(db), encoding)?;

    // Normalize the new name to its canonical R identifier syntax
    // (backtick-wrapped if needed) up front, so an invalid name fails fast. Use
    // sites are always bare identifiers, so the name must be a valid identifier
    // regardless of how any one site is spelled.
    let identifier_text = to_identifier_text(&new_name)?;
    let ranges = oak_ide::rename(db, file, offset)?;

    let mut changes: HashMap<lsp_types::Uri, Vec<TextEdit>> = HashMap::new();
    for site in ranges {
        let line_index = site.file.line_index(db);
        let path = site.file.path(db);

        // A rename is all or nothing. Skipping a file here would rename some
        // uses of the symbol and silently leave the rest behind, so we bail
        // instead.
        let target_uri = state
            .wire_uri(site.file)
            .with_context(|| format!("Can't rename: no valid URI for `{path}`."))?;

        // A site spelled as a quoted string is a string-form binding
        // (`assign("x", ..)`, `"x" <- ..`). Rename it in place, keeping the
        // quotes, rather than unquoting it: dropping the quotes on an
        // `assign()` argument would turn the name into a variable reference and
        // change the program.
        let source = site.file.source_text(db);
        let new_text = match string_delimiter(source, site.range) {
            Some(delimiter) => quote_name(&new_name, delimiter),
            None => identifier_text.clone(),
        };

        let range = to_proto::range(site.range, line_index, encoding)
            .with_context(|| format!("Can't rename: no valid text range in `{path}`."))?;

        changes
            .entry(target_uri)
            .or_default()
            .push(TextEdit { range, new_text });
    }

    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }))
}

/// The opening quote of the string literal at `range` in `source`, or `None`
/// when the site is a bare identifier. A string-form name binding renders its
/// name as a quoted argument, so its rename edit has to stay quoted.
fn string_delimiter(source: &str, range: TextRange) -> Option<char> {
    let slice = &source[usize::from(range.start())..usize::from(range.end())];
    match slice.chars().next() {
        Some(delimiter @ ('"' | '\'')) => Some(delimiter),
        _ => None,
    }
}

/// Render `name` as a quoted string literal using `delimiter`. Identifiers
/// don't contain quotes or backslashes, but we escape defensively so an unusual
/// name can't break out of the string.
fn quote_name(name: &str, delimiter: char) -> String {
    let escaped = name
        .replace('\\', "\\\\")
        .replace(delimiter, &format!("\\{delimiter}"));
    format!("{delimiter}{escaped}{delimiter}")
}
