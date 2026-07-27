use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use oak_db::Db;
use stdext::result::ResultExt;
use tower_lsp_server::ls_types::Location;
use tower_lsp_server::ls_types::ReferenceParams;

use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

pub(crate) fn find_references(
    params: ReferenceParams,
    state: &WorldState,
) -> anyhow::Result<Vec<Location>> {
    let path = params
        .text_document_position
        .text_document
        .uri
        .to_document_path()?;
    let position = params.text_document_position.position;
    let include_declaration = params.context.include_declaration;

    let db = &state.db;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_path(&path) else {
        return Ok(Vec::new());
    };

    let offset = from_proto::offset_from_position(position, file.line_index(db), encoding)?;
    let file_ranges = oak_ide::find_references(db, file, offset, include_declaration);

    // A reference we can't convert is dropped rather than failing the whole
    // request, so the user still sees the ones we could resolve.
    let locations = file_ranges
        .iter()
        .filter_map(|file_range| {
            let line_index = file_range.file.line_index(db);
            let range = to_proto::range(file_range.range, line_index, encoding).log_err()?;
            let uri = state.wire_uri(file_range.file).log_err()?;
            Some(Location::new(uri, range))
        })
        .collect();

    Ok(locations)
}
