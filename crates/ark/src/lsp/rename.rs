use std::collections::HashMap;

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_path::FilePath;
use oak_db::Db;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::PrepareRenameResponse;
use tower_lsp::lsp_types::RenameParams;
use tower_lsp::lsp_types::TextDocumentPositionParams;
use tower_lsp::lsp_types::TextEdit;
use tower_lsp::lsp_types::WorkspaceEdit;

use crate::lsp::state::WorldState;

pub(crate) fn prepare_rename(
    params: TextDocumentPositionParams,
    state: &WorldState,
) -> anyhow::Result<Option<PrepareRenameResponse>> {
    let uri = params.text_document.uri;
    let position = params.position;

    let db = &state.oak;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_url(&FilePath::from_url(&uri)) else {
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
    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let new_name = params.new_name;

    let db = &state.oak;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_url(&FilePath::from_url(&uri)) else {
        return Ok(None);
    };

    let offset = from_proto::offset_from_position(position, file.line_index(db), encoding)?;

    let targets = oak_ide::rename(db, file, offset, &new_name)?;

    let mut changes: HashMap<lsp_types::Url, Vec<TextEdit>> = HashMap::new();
    for r in targets.ranges {
        let line_index = r.file.line_index(db);
        let target_url = r.file.url(db).to_url();
        let range = to_proto::range(r.range, line_index, encoding)?;
        changes.entry(target_url).or_default().push(TextEdit {
            range,
            new_text: targets.new_text.clone(),
        });
    }

    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }))
}
