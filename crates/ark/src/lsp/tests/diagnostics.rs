use aether_path::FilePath;
use oak_scan::DbScan;
use url::Url;

use crate::lsp::diagnostics::generate_diagnostics;
use crate::lsp::state::WorldState;
use crate::r_task;

#[test]
fn test_diagnostics_published_through_refresh_snapshot() {
    let mut state = WorldState::default();

    // A tighter scope for `r_task()` results in a compilation error about
    // sharing Salsa ingredients across threads
    let diagnostics = r_task(|| {
        // Open an editor file with an undefined symbol, mirroring `did_open`:
        // a `Document` plus its matching `oak_db::File`.
        let uri = Url::parse("file:///test.R").unwrap();
        let code = "foo";
        let file = state
            .db
            .upsert_editor(FilePath::from_url(&uri), code.to_string());
        state.insert_ark_file(uri.clone(), file, None);

        // Mirror `diagnostics_refresh_all`: build the `ArkFile` from the live
        // state, then hand the worker the `diagnostics_snapshot`. The snapshot's
        // oak must still serve the file the `ArkFile` points at.
        let ark_file = state
            .ark_file(&uri)
            .expect("ArkFile builds from live state");

        let snapshot = state.diagnostics_snapshot();
        generate_diagnostics(ark_file, snapshot, false)
    });

    assert!(!diagnostics.is_empty());
}
