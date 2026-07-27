use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use oak_db::Db;
use oak_ide::NavigationTarget;
use stdext::result::ResultExt;
use tower_lsp_server::ls_types::GotoDefinitionParams;
use tower_lsp_server::ls_types::GotoDefinitionResponse;
use tower_lsp_server::ls_types::LocationLink;

use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

pub(crate) fn goto_definition(
    params: GotoDefinitionParams,
    state: &WorldState,
) -> anyhow::Result<Option<GotoDefinitionResponse>> {
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .to_document_path()?;
    let position = params.text_document_position_params.position;

    let db = &state.db;
    let encoding = state.config.position_encoding;

    let Some(file) = db.file_by_path(&path) else {
        return Ok(None);
    };

    let offset = from_proto::offset_from_position(position, file.line_index(db), encoding)?;

    let targets = oak_ide::goto_definition(db, file, offset);
    if targets.is_empty() {
        return Ok(None);
    }

    // An ambiguous name (e.g. defined on both arms of an `if`/`else`) resolves
    // to several bindings. A target we can't convert is dropped rather than
    // failing the whole request, so the user can see the definitions we did
    // resolve.
    let links: Vec<LocationLink> = targets
        .iter()
        .filter_map(|target| nav_target_to_link(state, encoding, target).log_err())
        .collect();

    if links.is_empty() {
        return Ok(None);
    }

    Ok(Some(GotoDefinitionResponse::Link(links)))
}

/// Convert a [`NavigationTarget`] into a `LocationLink`. Its ranges are byte
/// offsets in the target file, so we translate them through that file's own
/// line index, not the file the request came from.
fn nav_target_to_link(
    state: &WorldState,
    encoding: PositionEncoding,
    target: &NavigationTarget,
) -> anyhow::Result<LocationLink> {
    let db = &state.db;
    let line_index = target.file.line_index(db);
    let target_range = to_proto::range(target.full_range, line_index, encoding)?;
    let target_selection_range = to_proto::range(target.focus_range, line_index, encoding)?;

    Ok(LocationLink {
        origin_selection_range: None,
        target_uri: state.wire_uri(target.file)?,
        target_range,
        target_selection_range,
    })
}
