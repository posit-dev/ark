use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use aether_path::FilePath;
use oak_db::Db;
use oak_ide::NavigationTarget;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use tower_lsp::lsp_types::LocationLink;

use crate::lsp::state::WorldState;

pub(crate) fn goto_definition(
    params: GotoDefinitionParams,
    state: &WorldState,
) -> anyhow::Result<Option<GotoDefinitionResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let db = &state.db;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_path(&FilePath::from_url(uri)) else {
        return Ok(None);
    };

    let offset = from_proto::offset_from_position(position, file.line_index(db), encoding)?;

    let targets = oak_ide::goto_definition(db, file, offset);
    if targets.is_empty() {
        return Ok(None);
    }

    // An ambiguous name (e.g. defined on both arms of an `if`/`else`) resolves
    // to several bindings; the client offers all of them.
    let links = targets
        .iter()
        .map(|target| nav_target_to_link(db, encoding, target))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(GotoDefinitionResponse::Link(links)))
}

/// Convert a [`NavigationTarget`] into a `LocationLink`. Its ranges are byte
/// offsets in the target file, so we translate them through that file's own
/// line index, not the file the request came from.
fn nav_target_to_link(
    db: &dyn Db,
    encoding: PositionEncoding,
    target: &NavigationTarget,
) -> anyhow::Result<LocationLink> {
    let line_index = target.file.line_index(db);
    let target_range = to_proto::range(target.full_range, line_index, encoding)?;
    let target_selection_range = to_proto::range(target.focus_range, line_index, encoding)?;

    Ok(LocationLink {
        origin_selection_range: None,
        target_uri: target.file.path(db).to_url(),
        target_range,
        target_selection_range,
    })
}
