use aether_path::FilePath;
use oak_scan::DbScan;
use url::Url;

use crate::lsp::diagnostics::generate_diagnostics;
use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UrlExt;
use crate::r_task;

#[test]
fn test_diagnostics_published_through_refresh_snapshot() {
    let mut state = WorldState::default();

    // A tighter scope for `r_task()` results in a compilation error about
    // sharing Salsa ingredients across threads
    let diagnostics = r_task(|| {
        // Open an editor file with an undefined symbol, mirroring `did_open`.
        // `upsert_editor` pushes the contents into the oak and returns the
        // matching `File`, which `insert_open_file` stores as an `OpenFile`.
        let url = Url::parse("file:///test.R").unwrap();
        let uri = url.to_uri().unwrap();
        let code = "foo";
        let file = state
            .db
            .upsert_editor(FilePath::from_url(&url), code.to_string());
        state.insert_open_file(uri.clone(), FilePath::from_url(&url), file, None);

        // Mirror `DiagnosticsState::refresh_all`: fetch the `File` from the
        // live state, then hand the worker a snapshot. The snapshot's oak must
        // still serve that file.
        let file = state
            .open_file(&FilePath::from_url(&url))
            .expect("file is open in live state")
            .file();

        let snapshot = state.snapshot();
        generate_diagnostics(file, snapshot, false, &uri)
    });

    assert!(!diagnostics.is_empty());
}
