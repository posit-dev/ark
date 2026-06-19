use aether_path::FilePath;
use oak_scan::DbScan;
use tower_lsp::lsp_types;

use crate::lsp::document::Document;
use crate::lsp::state::WorldState;

pub(super) fn make_state(uri: &lsp_types::Url, doc: &Document) -> WorldState {
    let mut state = WorldState::default();
    insert_file(&mut state, uri, doc);
    state
}

/// Insert a document and mirror its contents into `oak`, the same pair
/// `did_open` performs, so handlers reading either `state.documents` or
/// `state.oak` (via `file_by_url`) see a consistent file.
pub(super) fn insert_file(state: &mut WorldState, uri: &lsp_types::Url, doc: &Document) {
    state.insert_document(uri.clone(), doc.clone());
    state
        .db
        .upsert_editor(FilePath::from_url(uri), doc.contents.clone());
}

pub(super) fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position::new(start.0, start.1),
        end: lsp_types::Position::new(end.0, end.1),
    }
}
