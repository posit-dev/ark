//! Smoke tests for the LSP -> oak translator in [`crate::lsp::state_handlers`].
//! Dispatch behaviour itself is covered by `oak_scan/tests/watch.rs`. The tests
//! here go through [`did_change_watched_files`] end-to-end so they catch a
//! regression in either the translation step or the state.documents → skip set
//! conversion.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_scan::DbExt;
use tower_lsp::lsp_types::DidChangeWatchedFilesParams;
use tower_lsp::lsp_types::DidChangeWorkspaceFoldersParams;
use tower_lsp::lsp_types::DidCloseTextDocumentParams;
use tower_lsp::lsp_types::FileChangeType;
use tower_lsp::lsp_types::FileEvent;
use tower_lsp::lsp_types::InitializeParams;
use tower_lsp::lsp_types::TextDocumentIdentifier;
use tower_lsp::lsp_types::WorkspaceFolder;
use tower_lsp::lsp_types::WorkspaceFoldersChangeEvent;
use url::Url;

use crate::lsp::capabilities::Capabilities;
use crate::lsp::document::Document;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::LspState;
use crate::lsp::state::WorldState;
use crate::lsp::state_handlers::did_change_watched_files;
use crate::lsp::state_handlers::did_change_workspace_folders;
use crate::lsp::state_handlers::did_close;
use crate::lsp::state_handlers::effective_workspace_uris;

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
    state
        .workspace
        .folders
        .push(Url::from_file_path(workspace).unwrap());
    state
        .oak
        .set_workspace_paths(&[workspace.to_path_buf()], &HashSet::new());
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
        .upsert_editor(url_id.clone(), "editor_v2\n".to_string());

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

#[test]
fn test_r_file_changed_for_unopened_file_updates_contents() {
    // No editor buffer, so the watcher's disk content is authoritative
    // and should land in `file.contents()`. Complements the open-file
    // skip test above.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.R");
    fs::write(&path, "v1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let url_id = UrlId::from_file_path(&path).unwrap();
    assert_eq!(
        state.oak.file_by_url(&url_id).unwrap().contents(&state.oak),
        "v1\n"
    );

    fs::write(&path, "v2\n").unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::CHANGED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    assert_eq!(
        state.oak.file_by_url(&url_id).unwrap().contents(&state.oak),
        "v2\n"
    );
}

#[test]
#[ignore = "blocked by UrlId canonicalization gap for missing paths"]
fn test_r_file_deleted_for_editor_open_file_is_skipped() {
    // Mirror of `test_r_file_changed_for_editor_open_file_is_skipped`
    // for the Deleted kind. The skip check in `apply_watcher_events`
    // sits before the kind match, so editor-owned URLs should be
    // protected from deletion too: the buffer stays visible to the
    // user and queries keep resolving against the editor's
    // last-pushed content.
    //
    // The assertion passes today, but only incidentally: the same
    // canonicalization gap that blocks the DESCRIPTION test below also
    // breaks the skip lookup here. The event URL canonicalizes to the
    // raw `/var/...` path (the file is gone, canonicalize fails),
    // so the skip set (built from open documents while the file
    // existed, i.e. `/private/var/...`) does not contain it. The event
    // therefore reaches `remove_watched_file`, which also fails to find
    // the file by URL and bails out. Net: file survives, but the skip
    // mechanism is never actually exercised. Marked ignore until the
    // canonicalization gap is fixed.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.R");
    fs::write(&path, "disk_v1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let url = Url::from_file_path(&path).unwrap();
    state
        .documents
        .insert(url.clone(), Document::new("editor_v2\n", None));
    let url_id = UrlId::from_url(url.clone());
    state
        .oak
        .upsert_editor(url_id.clone(), "editor_v2\n".to_string());

    fs::remove_file(&path).unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::DELETED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let file = state.oak.file_by_url(&url_id).unwrap();
    assert_eq!(file.contents(&state.oak), "editor_v2\n");
}

#[test]
#[ignore = "blocked by UrlId canonicalization gap for missing paths; see dev-notes/notes/oak/2026-05-22-0846-watcher-test-gaps.md"]
fn test_description_deleted_demotes_package_to_scripts() {
    // Inverse of `test_description_created_triggers_root_rescan`: a
    // DESCRIPTION removed mid-session triggers a root rescan and the
    // former package's R/ files surface as workspace scripts.
    //
    // Currently fails because `UrlId::from_url` canonicalizes via
    // `std::fs::canonicalize`, which fails for paths that no longer
    // exist and falls back to the raw path. On macOS this means the
    // Deleted event's URL keeps the `/var/...` prefix while the
    // workspace root URL uses `/private/var/...`, and the routing in
    // `apply_watcher_events` misses.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut state = workspace_state(tmp.path());

    let root = state.oak.workspace_roots().roots(&state.oak)[0];
    assert_eq!(root.packages(&state.oak).len(), 1);
    assert!(root.scripts(&state.oak).is_empty());

    fs::remove_file(tmp.path().join("pkg/DESCRIPTION")).unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(
            &tmp.path().join("pkg/DESCRIPTION"),
            FileChangeType::DELETED,
        )],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let root = state.oak.workspace_roots().roots(&state.oak)[0];
    assert!(root.packages(&state.oak).is_empty());
    assert_eq!(root.scripts(&state.oak).len(), 1);

    let a_url = UrlId::from_file_path(tmp.path().join("pkg/R/a.R")).unwrap();
    let file = state.oak.file_by_url(&a_url).unwrap();
    assert_eq!(file.package(&state.oak), None);
}

fn folder(uri: &str) -> WorkspaceFolder {
    WorkspaceFolder {
        uri: Url::parse(uri).unwrap(),
        name: String::new(),
    }
}

#[test]
fn test_effective_workspace_uris_reads_workspace_folders() {
    let params = InitializeParams {
        workspace_folders: Some(vec![folder("file:///a"), folder("file:///b")]),
        ..Default::default()
    };
    let uris = effective_workspace_uris(&params);
    assert_eq!(uris.len(), 2);
    assert_eq!(uris[0].as_str(), "file:///a");
    assert_eq!(uris[1].as_str(), "file:///b");
}

#[test]
fn test_effective_workspace_uris_ignores_legacy_root_uri() {
    // We dropped the `root_uri` fallback, so a client sending only the
    // deprecated field gets single-file mode (empty), whether
    // `workspace_folders` is absent or an empty list.
    let absent = InitializeParams {
        workspace_folders: None,
        root_uri: Some(Url::parse("file:///legacy").unwrap()),
        ..Default::default()
    };
    assert!(effective_workspace_uris(&absent).is_empty());

    let empty = InitializeParams {
        workspace_folders: Some(vec![]),
        root_uri: Some(Url::parse("file:///legacy").unwrap()),
        ..Default::default()
    };
    assert!(effective_workspace_uris(&empty).is_empty());
}

#[test]
fn test_effective_workspace_uris_single_file_mode() {
    let params = InitializeParams {
        workspace_folders: None,
        root_uri: None,
        ..Default::default()
    };
    assert!(effective_workspace_uris(&params).is_empty());
}

fn folders_change(
    added: Vec<WorkspaceFolder>,
    removed: Vec<WorkspaceFolder>,
) -> DidChangeWorkspaceFoldersParams {
    DidChangeWorkspaceFoldersParams {
        event: WorkspaceFoldersChangeEvent { added, removed },
    }
}

fn folder_for(path: &Path) -> WorkspaceFolder {
    WorkspaceFolder {
        uri: Url::from_file_path(path).unwrap(),
        name: String::new(),
    }
}

#[test]
fn test_did_change_workspace_folders_adds_new_folder() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_package(&first.path().join("pkg1"), "pkg1", &[]);
    write_package(&second.path().join("pkg2"), "pkg2", &[]);

    let mut state = workspace_state(first.path());
    assert_eq!(state.workspace.folders.len(), 1);
    assert_eq!(state.oak.workspace_roots().roots(&state.oak).len(), 1);

    let params = folders_change(vec![folder_for(second.path())], vec![]);
    did_change_workspace_folders(params, &mut state).unwrap();

    assert_eq!(state.workspace.folders.len(), 2);
    let roots = state.oak.workspace_roots().roots(&state.oak).clone();
    assert_eq!(roots.len(), 2);
    // Existing root stays first, new one appended.
    assert_eq!(roots[0].packages(&state.oak)[0].name(&state.oak), "pkg1");
    assert_eq!(roots[1].packages(&state.oak)[0].name(&state.oak), "pkg2");
}

#[test]
fn test_did_change_workspace_folders_removes_folder() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_package(&first.path().join("pkg1"), "pkg1", &[]);
    write_package(&second.path().join("pkg2"), "pkg2", &[]);

    let mut state = WorldState::default();
    state
        .workspace
        .folders
        .push(Url::from_file_path(first.path()).unwrap());
    state
        .workspace
        .folders
        .push(Url::from_file_path(second.path()).unwrap());
    state.oak.set_workspace_paths(
        &[first.path().to_path_buf(), second.path().to_path_buf()],
        &HashSet::new(),
    );
    assert_eq!(state.oak.workspace_roots().roots(&state.oak).len(), 2);

    let params = folders_change(vec![], vec![folder_for(first.path())]);
    did_change_workspace_folders(params, &mut state).unwrap();

    assert_eq!(state.workspace.folders.len(), 1);
    let roots = state.oak.workspace_roots().roots(&state.oak).clone();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].packages(&state.oak)[0].name(&state.oak), "pkg2");
}

#[test]
fn test_did_change_workspace_folders_ignores_duplicate_add() {
    // A client that re-announces a folder that's already tracked
    // shouldn't end up with two copies in `state.workspace.folders`
    // or two `Root` entries in oak.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[]);
    let mut state = workspace_state(tmp.path());

    let params = folders_change(vec![folder_for(tmp.path())], vec![]);
    did_change_workspace_folders(params, &mut state).unwrap();

    assert_eq!(state.workspace.folders.len(), 1);
    assert_eq!(state.oak.workspace_roots().roots(&state.oak).len(), 1);
}

#[test]
fn test_did_change_workspace_folders_handles_add_and_remove_in_one_event() {
    // The LSP sends both `added` and `removed` in a single event when the
    // user swaps one folder for another.
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_package(&first.path().join("pkg1"), "pkg1", &[]);
    write_package(&second.path().join("pkg2"), "pkg2", &[]);
    let mut state = workspace_state(first.path());

    let params = folders_change(vec![folder_for(second.path())], vec![folder_for(
        first.path(),
    )]);
    did_change_workspace_folders(params, &mut state).unwrap();

    assert_eq!(state.workspace.folders.len(), 1);
    let roots = state.oak.workspace_roots().roots(&state.oak).clone();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].packages(&state.oak)[0].name(&state.oak), "pkg2");
}

#[test]
fn test_did_change_workspace_folders_preserves_open_buffer_across_churn() {
    // End-to-end check that `did_change_workspace_folders` threads
    // `state.documents` URLs through as `editor_owned` so an open buffer
    // survives its workspace folder being removed and re-added: same
    // `File` entity, editor contents preserved, file findable through
    // both phases.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut state = workspace_state(tmp.path());

    // Simulate `didOpen` on the package file with editor-side content.
    let r_path = tmp.path().join("pkg/R/a.R");
    let url = Url::from_file_path(&r_path).unwrap();
    let url_id = UrlId::from_url(url.clone());
    state
        .documents
        .insert(url.clone(), Document::new("editor <- 2\n", None));
    state
        .oak
        .upsert_editor(url_id.clone(), "editor <- 2\n".to_string());

    let file_before = state.oak.file_by_url(&url_id).unwrap();

    // Remove the workspace folder. The handler builds the editor_owned set
    // from state.documents.keys() and passes it to oak; the buffer's file
    // routes to OrphanRoot rather than StaleRoot.
    let params = folders_change(vec![], vec![folder_for(tmp.path())]);
    did_change_workspace_folders(params, &mut state).unwrap();

    let after_remove = state.oak.file_by_url(&url_id).unwrap();
    assert_eq!(file_before, after_remove);
    assert_eq!(after_remove.package(&state.oak), None);
    assert!(state
        .oak
        .orphan_root()
        .files(&state.oak)
        .contains(&after_remove));
    assert_eq!(after_remove.contents(&state.oak), "editor <- 2\n");

    // Re-add the same folder. The file snaps back into pkg.files with
    // the same entity and the editor content carries over (the scan's
    // disk snapshot doesn't overwrite).
    let params = folders_change(vec![folder_for(tmp.path())], vec![]);
    did_change_workspace_folders(params, &mut state).unwrap();

    let after_readd = state.oak.file_by_url(&url_id).unwrap();
    assert_eq!(file_before, after_readd);
    assert!(after_readd.package(&state.oak).is_some());
    assert_eq!(after_readd.contents(&state.oak), "editor <- 2\n");
    // `upsert_root_file` cleaned the orphan reference.
    assert!(!state
        .oak
        .orphan_root()
        .files(&state.oak)
        .contains(&after_readd));
}

#[test]
fn test_did_close_releases_orphan_file_to_stale() {
    // End-to-end: open buffer → remove its workspace folder (file goes
    // to orphan, editor-owned) → close → file leaves orphan, lands in
    // stale. Without the `close_editor` hook in `did_close`, the file
    // would zombie in orphan with the editor's last content.
    init_aux_for_test();

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut state = workspace_state(tmp.path());
    let mut lsp_state = LspState {
        parsers: HashMap::new(),
        capabilities: Capabilities::default(),
        console_notification_tx: tokio::sync::mpsc::unbounded_channel().0,
    };

    let r_path = tmp.path().join("pkg/R/a.R");
    let url = Url::from_file_path(&r_path).unwrap();
    let url_id = UrlId::from_url(url.clone());

    // Simulate `didOpen` via state mutation (matches the rest of the file's
    // pattern).
    state
        .documents
        .insert(url.clone(), Document::new("edited\n", None));
    lsp_state
        .parsers
        .insert(url.clone(), tree_sitter::Parser::new());
    state
        .oak
        .upsert_editor(url_id.clone(), "edited\n".to_string());

    // Remove the workspace folder; file goes to orphan (editor-owned).
    did_change_workspace_folders(
        folders_change(vec![], vec![folder_for(tmp.path())]),
        &mut state,
    )
    .unwrap();
    let file = state.oak.file_by_url(&url_id).unwrap();
    assert!(state.oak.orphan_root().files(&state.oak).contains(&file));

    // Now close the buffer. File should move from orphan to stale.
    let params = DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: url.clone() },
    };
    did_close(params, &mut lsp_state, &mut state).unwrap();

    assert!(!state.oak.orphan_root().files(&state.oak).contains(&file));
    assert!(state.oak.stale_root().files(&state.oak).contains(&file));
}
