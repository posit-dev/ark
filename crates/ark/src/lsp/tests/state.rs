use aether_path::FilePath;
use oak_db::File;
use oak_scan::DbScan;
use tower_lsp_server::ls_types::Uri;
use url::Url;

use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

/// Register an open buffer the way `did_open` does, starting from the bytes an
/// editor would put on the wire rather than from a `Url`. Starting from a `Url`
/// would hide any normalisation the `Uri` -> `Url` conversion applies.
fn open_buffer_from_wire(state: &mut WorldState, wire: &str) -> File {
    let uri: Uri = wire.parse().unwrap();
    let url = uri.to_url().unwrap();
    let file = state
        .db
        .upsert_editor(FilePath::from_url(&url), "x <- 1\n".to_string());
    state.insert_open_file(uri, FilePath::from_url(&url), file, None);
    file
}

#[test]
fn test_wire_uri_non_open_file_synthesises_uri() {
    let mut state = WorldState::default();
    let url = Url::parse("file:///C:/proj//bar.R").unwrap();
    let file = state
        .db
        .upsert_editor(FilePath::from_url(&url), "y <- 2\n".to_string());
    // Not inserted into open_files, so wire_uri synthesises from the
    // normalised path (dropping the doubled slash) and encodes the drive
    // colon the way `Uri` requires, rather than replaying the original bytes.
    let wire = state.wire_uri(file).unwrap();
    assert_eq!(wire.as_str(), "file:///C%3A/proj/bar.R");
    assert_ne!(wire.as_str(), url.as_str());
}

#[test]
fn test_wire_uri_open_buffer_replays_editor_bytes() {
    // The editor matches a response to its document by URI, so what we send
    // back has to be byte-identical to what it sent us. Each of these survives
    // `FilePath` normalisation only because the buffer stashes the verbatim URI.
    for wire in [
        "file:///C:/proj//foo.R",     // doubled slash
        "file:///c%3A/proj/foo.R",    // percent-encoded drive colon
        "file:///C:/proj/a%20b.R",    // encoded space
        "file:///C:/proj/f%5B1%5D.R", // encoded brackets
        "untitled:Untitled-1",
    ] {
        let mut state = WorldState::default();
        let file = open_buffer_from_wire(&mut state, wire);
        assert_eq!(state.wire_uri(file).unwrap().as_str(), wire);
    }
}

#[test]
fn test_wire_uri_non_open_file_encodes_reserved_characters() {
    // `[` and `]` are legal in a filename but reserved in a URI. `Url` leaves
    // them raw in a path, so a file like `f[1].R` that the editor never opened
    // has no `Url`-based route we could turn into a `Uri` at all.
    let mut state = WorldState::default();
    let path = FilePath::parse("file:///C:/proj/f%5B1%5D.R").unwrap();
    let file = state.db.upsert_editor(path, "y <- 2\n".to_string());

    // Not open, so this takes the `Uri::from_file_path()` fallback.
    let uri = state.wire_uri(file).unwrap();
    assert_eq!(uri.as_str(), "file:///C%3A/proj/f%5B1%5D.R");

    // Confirm a `Url`-based route really would have failed, so the check
    // above bites.
    let disk_path = file.path(&state.db).as_path().unwrap();
    assert!(Url::from_file_path(disk_path)
        .unwrap()
        .as_str()
        .parse::<Uri>()
        .is_err());
}
