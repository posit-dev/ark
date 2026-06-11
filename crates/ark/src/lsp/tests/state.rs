use aether_path::FilePath;
use oak_scan::DbScan;
use url::Url;

use crate::lsp::state::WorldState;

#[test]
fn test_wire_url_open_buffer_keeps_verbatim_url() {
    let mut state = WorldState::default();
    let url = Url::parse("file:///C:/proj//foo.R").unwrap();
    let file = state
        .db
        .upsert_editor(FilePath::from_url(&url), "x <- 1\n".to_string());
    state.insert_ark_file(url.clone(), file, None);
    assert_eq!(state.wire_url(file), url);
}

#[test]
fn test_wire_url_non_open_file_synthesises_url() {
    let mut state = WorldState::default();
    let url = Url::parse("file:///C:/proj//bar.R").unwrap();
    let file = state
        .db
        .upsert_editor(FilePath::from_url(&url), "y <- 2\n".to_string());
    // Not inserted into open_files, so wire_url synthesises from path
    let wire = state.wire_url(file);
    assert_eq!(wire, file.path(&state.db).to_url());
    assert_ne!(wire, url);
}
