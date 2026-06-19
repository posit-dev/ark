use aether_path::FilePath;
use oak_scan::DbScan;
use tower_lsp::lsp_types;

use crate::lsp::state::WorldState;

pub(super) fn make_state(uri: &lsp_types::Url, contents: &str) -> WorldState {
    let mut state = WorldState::default();
    insert_file(&mut state, uri, contents);
    state
}

/// Insert an editor buffer, the same as `did_open` performs, so handlers
/// reading either `state.documents` or `state.db` (via `file_by_path`) see a
/// consistent file.
pub(super) fn insert_file(state: &mut WorldState, uri: &lsp_types::Url, contents: &str) {
    let file = state
        .db
        .upsert_editor(FilePath::from_url(uri), contents.to_string());
    state.insert_ark_file(uri.clone(), file, None);
}

pub(super) fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position::new(start.0, start.1),
        end: lsp_types::Position::new(end.0, end.1),
    }
}
