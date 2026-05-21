//! Smoke tests for the LSP -> oak translator in [`crate::lsp::state_handlers`].
//! Dispatch behaviour itself is covered by `oak_scan/tests/watch.rs`. The tests
//! here go through [`did_change_watched_files`] end-to-end so they catch a
//! regression in either the translation step or the state.documents → skip set
//! conversion.

use std::fs;
use std::path::Path;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_scan::DbExt;
use tower_lsp::lsp_types::DidChangeWatchedFilesParams;
use tower_lsp::lsp_types::FileChangeType;
use tower_lsp::lsp_types::FileEvent;
use url::Url;

use crate::lsp::document::Document;
use crate::lsp::state::WorldState;
use crate::lsp::state_handlers::did_change_watched_files;

fn write_package(dir: &Path, name: &str, r_files: &[(&str, &str)]) {
    fs::create_dir_all(dir.join("R")).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 0.0.0\n"),
    )
    .unwrap();
    for (basename, contents) in r_files {
        fs::write(dir.join("R").join(basename), contents).unwrap();
    }
}

fn event(path: &Path, typ: FileChangeType) -> FileEvent {
    FileEvent {
        uri: Url::from_file_path(path).unwrap(),
        typ,
    }
}

fn workspace_state(workspace: &Path) -> WorldState {
    let mut state = WorldState::default();
    state.oak.scan_workspace_paths(&[workspace.to_path_buf()]);
    state
}

#[test]
fn test_description_created_triggers_root_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    fs::write(tmp.path().join("pkg/R/a.R"), "x <- 1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    // No DESCRIPTION yet, so `a.R` registers as a workspace script.
    let root = state.oak.workspace_roots().roots(&state.oak)[0];
    assert!(root.packages(&state.oak).is_empty());

    // Now write DESCRIPTION and fire the watcher.
    fs::write(
        tmp.path().join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 0.0.0\n",
    )
    .unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(
            &tmp.path().join("pkg/DESCRIPTION"),
            FileChangeType::CREATED,
        )],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let root = state.oak.workspace_roots().roots(&state.oak)[0];
    assert_eq!(root.packages(&state.oak).len(), 1);
    assert_eq!(root.packages(&state.oak)[0].name(&state.oak), "pkg");
}

#[test]
fn test_multiple_descriptions_under_same_root_dedup_to_one_rescan() {
    // Two DESCRIPTION events in the same root should not double-scan.
    // We can't observe the dedup directly, but we can check the end
    // state is consistent and the call doesn't error.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg1"), "pkg1", &[]);
    write_package(&tmp.path().join("pkg2"), "pkg2", &[]);
    let mut state = workspace_state(tmp.path());

    let params = DidChangeWatchedFilesParams {
        changes: vec![
            event(
                &tmp.path().join("pkg1/DESCRIPTION"),
                FileChangeType::CHANGED,
            ),
            event(
                &tmp.path().join("pkg2/DESCRIPTION"),
                FileChangeType::CHANGED,
            ),
        ],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let root = state.oak.workspace_roots().roots(&state.oak)[0];
    assert_eq!(root.packages(&state.oak).len(), 2);
}

#[test]
fn test_r_file_created_routes_through_add_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = workspace_state(tmp.path());

    let path = tmp.path().join("new.R");
    fs::write(&path, "x <- 1\n").unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::CREATED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let root = state.oak.workspace_roots().roots(&state.oak)[0];
    assert_eq!(root.scripts(&state.oak).len(), 1);
    let url = UrlId::from_file_path(&path).unwrap();
    let file = state.oak.file_by_url(&url).unwrap();
    assert_eq!(file.contents(&state.oak), "x <- 1\n");
}

#[test]
fn test_r_file_changed_for_editor_open_file_is_skipped() {
    // The editor is the source of truth for files it has open.
    // A disk-side `Changed` event that races against `didChange`
    // must not overwrite the editor's content.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.R");
    fs::write(&path, "disk_v1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let url = Url::from_file_path(&path).unwrap();
    state
        .documents
        .insert(url.clone(), Document::new("editor_v2\n", None));
    // Pretend the editor pushed its content into oak too.
    let url_id = UrlId::from_url(url.clone());
    state
        .oak
        .set_editor_contents(url_id.clone(), "editor_v2\n".to_string());

    // Now disk-side `Changed` fires with stale disk content.
    fs::write(&path, "disk_v3\n").unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::CHANGED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let file = state.oak.file_by_url(&url_id).unwrap();
    assert_eq!(file.contents(&state.oak), "editor_v2\n");
}

#[test]
fn test_r_file_deleted_routes_through_remove_file() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.R"), "x <- 1\n").unwrap();
    fs::write(tmp.path().join("b.R"), "y <- 2\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let path = tmp.path().join("a.R");
    let url_id = UrlId::from_file_path(&path).unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::DELETED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let root = state.oak.workspace_roots().roots(&state.oak)[0];
    assert_eq!(root.scripts(&state.oak).len(), 1);
    assert!(state.oak.file_by_url(&url_id).is_none());
}
