use std::collections::HashMap;

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
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
    let document = state.get_document(&uri)?;

    let offset = from_proto::offset_from_position(
        position,
        &document.line_index,
        document.position_encoding,
    )?;
    let index = document.semantic_index();
    let tree = document.syntax()?;
    let pos = oak_ide::FileOffset { file: uri, offset };

    let Some((range, placeholder)) = oak_ide::prepare_rename(&index, &tree, &pos) else {
        return Ok(None);
    };

    let range = to_proto::range(range, &document.line_index, document.position_encoding)?;
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
    let document = state.get_document(&uri)?;

    let offset = from_proto::offset_from_position(
        position,
        &document.line_index,
        document.position_encoding,
    )?;
    let index = document.semantic_index();
    let root = document.syntax()?;
    let pos = oak_ide::FileOffset {
        file: uri.clone(),
        offset,
    };

    let targets = oak_ide::rename(&index, &root, &pos, &new_name)?;

    // All edits target the current file (intra-file rename).
    let mut edits: Vec<TextEdit> = Vec::with_capacity(targets.ranges.len());
    for r in targets.ranges {
        let range = to_proto::range(r.range, &document.line_index, document.position_encoding)?;
        edits.push(TextEdit {
            range,
            new_text: targets.new_text.clone(),
        });
    }

    let mut changes: HashMap<lsp_types::Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri, edits);

    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }))
}
