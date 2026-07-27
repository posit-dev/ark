use std::collections::HashMap;

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use anyhow::Context;
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

    // Normalize the new name to its canonical R syntax (backtick-wrapped if
    // needed) before searching, so an invalid name fails fast.
    let new_text = to_identifier_text(&new_name)?;
    let ranges = oak_ide::rename(db, file, offset)?;

    let mut changes: HashMap<lsp_types::Uri, Vec<TextEdit>> = HashMap::new();
    for file_range in ranges {
        let line_index = file_range.file.line_index(db);
        let path = file_range.file.path(db);

        // A rename is all or nothing. Skipping a file here would rename some
        // uses of the symbol and silently leave the rest behind, so we bail
        // instead.
        let target_uri = state
            .wire_uri(file_range.file)
            .with_context(|| format!("Can't rename: no valid URI for `{path}`."))?;
        let range = to_proto::range(file_range.range, line_index, encoding)
            .with_context(|| format!("Can't rename: no valid text range in `{path}`."))?;

        changes.entry(target_uri).or_default().push(TextEdit {
            range,
            new_text: new_text.clone(),
        });
    }

    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }))
}
