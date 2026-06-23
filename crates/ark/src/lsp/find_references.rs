use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_path::FilePath;
use oak_db::Db;
use stdext::result::ResultExt;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::ReferenceParams;

use crate::lsp::state::WorldState;

pub(crate) fn find_references(
    params: ReferenceParams,
    state: &WorldState,
) -> anyhow::Result<Vec<Location>> {
    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let include_declaration = params.context.include_declaration;

    let db = &state.db;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_path(&FilePath::from_url(&uri)) else {
        return Ok(Vec::new());
    };

    let offset = from_proto::offset_from_position(position, file.line_index(db), encoding)?;
    let file_ranges = oak_ide::find_references(db, file, offset, include_declaration);

    let locations = file_ranges
        .iter()
        .filter_map(|fr| {
            let range = to_proto::range(fr.range, fr.file.line_index(db), encoding).log_err()?;
            Some(Location::new(state.wire_url(fr.file), range))
        })
        .collect();

    Ok(locations)
}
