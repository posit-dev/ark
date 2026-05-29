use tower_lsp::lsp_types;

use crate::lsp::document::Document;
use crate::lsp::state::WorldState;

pub(super) fn make_state(uri: &lsp_types::Url, doc: &Document) -> WorldState {
    let mut state = WorldState::default();
    state.insert_document(uri.clone(), doc.clone());
    state
}

pub(super) fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position::new(start.0, start.1),
        end: lsp_types::Position::new(end.0, end.1),
    }
}
