//! Smoke tests for the LSP -> oak translator in [`crate::lsp::state_handlers`].
//! Dispatch behaviour itself is covered by `oak_scan/tests/watch.rs`. The tests
//! here go through [`did_change_watched_files`] end-to-end so they catch a
//! regression in either the translation step or the handling of open buffers.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_path::AbsPathBuf;
use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::SourceDb;
use oak_scan::DbScan;
use oak_scan::ScanRequest;
use oak_scan::ScanScheduler;
use tower_lsp_server::ls_types::DidChangeTextDocumentParams;
use tower_lsp_server::ls_types::DidChangeWatchedFilesParams;
use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::DidCloseTextDocumentParams;
use tower_lsp_server::ls_types::DidOpenTextDocumentParams;
use tower_lsp_server::ls_types::FileChangeType;
use tower_lsp_server::ls_types::FileEvent;
use tower_lsp_server::ls_types::FileSystemWatcher;
use tower_lsp_server::ls_types::GlobPattern;
use tower_lsp_server::ls_types::InitializeParams;
use tower_lsp_server::ls_types::OneOf;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::Range;
use tower_lsp_server::ls_types::RelativePattern;
use tower_lsp_server::ls_types::TextDocumentContentChangeEvent;
use tower_lsp_server::ls_types::TextDocumentIdentifier;
use tower_lsp_server::ls_types::TextDocumentItem;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::VersionedTextDocumentIdentifier;
use tower_lsp_server::ls_types::WorkspaceFolder;
use tower_lsp_server::ls_types::WorkspaceFoldersChangeEvent;
use url::Url;

use super::utils::fixed_environments;
use crate::lsp::main_loop::dispatch_scan_requests;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::AuxiliaryEvent;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::state::WorldState;
use crate::lsp::state_handlers::did_change;
use crate::lsp::state_handlers::did_change_console_inputs;
use crate::lsp::state_handlers::did_close;
use crate::lsp::state_handlers::did_open;
use crate::lsp::state_handlers::effective_workspace_paths;
use crate::lsp::state_handlers::library_watchers;
use crate::lsp::state_handlers::ConsoleInputs;
use crate::lsp::traits::url::UriExt;
use crate::lsp::traits::url::UrlExt;

/// Local sync wrappers around the async-shaped scheduler API. Tests
/// don't need the timing flexibility, so each operation kicks off
/// any scans, drains them on the current thread, and returns. Each
/// call constructs a fresh `LspState` (which owns the scheduler)
/// because tests assert post-quiescent state; carrying scheduler
/// state across calls only matters for mid-flight timing assertions,
/// which live in `oak_scan`'s scheduler tests.
pub(crate) fn set_workspace_paths(state: &mut WorldState, paths: &[PathBuf]) {
    let mut lsp_state = test_lsp_state();
    let reqs = lsp_state
        .oak_scheduler
        .set_workspace_paths(state.db_mut(), paths);
    drain(state.db_mut(), &mut lsp_state.oak_scheduler, reqs);
}

fn did_change_watched_files(
    params: DidChangeWatchedFilesParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let mut lsp_state = test_lsp_state();
    run_handler_to_quiescence(state, &mut lsp_state, |state, lsp_state, events_tx| {
        crate::lsp::state_handlers::did_change_watched_files(params, state, lsp_state, events_tx)
    })
}

fn did_change_workspace_folders(
    params: DidChangeWorkspaceFoldersParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let mut lsp_state = test_lsp_state();
    run_handler_to_quiescence(state, &mut lsp_state, |state, lsp_state, events_tx| {
        crate::lsp::state_handlers::did_change_workspace_folders(
            params, state, lsp_state, events_tx,
        )
    })
}

/// Run a production handler and apply its scan and environment resolution
/// results until neither scheduler has pending work. This replaces the main
/// loop's event handling with a local Tokio runtime and channels, including
/// the auxiliary channel required by `send_auxiliary()`.
///
/// Applying the fixed resolver's results lets files in newly added workspace
/// folders receive semantic diagnostics instead of remaining syntax-only.
fn run_handler_to_quiescence<F>(
    state: &mut WorldState,
    lsp_state: &mut LspState,
    handler: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&mut WorldState, &mut LspState, &TokioUnboundedSender<Event>) -> anyhow::Result<()>,
{
    let _aux = init_aux_for_test();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    rt.block_on(async {
        handler(state, lsp_state, &events_tx)?;
        while lsp_state.oak_scheduler.has_pending_scans() ||
            lsp_state.environment_scheduler.has_pending()
        {
            match events_rx.recv().await {
                Some(Event::OakScanCompleted(scan)) => {
                    let followups = lsp_state
                        .oak_scheduler
                        .apply_scan_completed(state.db_mut(), scan);
                    dispatch_scan_requests(&lsp_state.scan_pool, &events_tx, followups);
                },
                Some(Event::EnvironmentResolved(completed)) => {
                    lsp_state.environment_scheduler.finish(
                        state.db_mut(),
                        completed,
                        &lsp_state.environment_pool,
                        &events_tx,
                    );
                },
                _ => panic!("Expected `OakScanCompleted` or `EnvironmentResolved`"),
            }
        }
        Ok(())
    })
}

fn test_lsp_state() -> LspState {
    LspState::new(tokio::sync::mpsc::unbounded_channel().0)
        .with_environment_scheduler(fixed_environments(&[]))
}

/// Inline drain loop: oak_scan keeps its equivalent crate-private so
/// it can't leak into production callers. The implementation here is
/// just `ScanRequest::run` + `apply_scan_completed` until the request queue
/// empties.
fn drain(
    db: &mut oak_db::OakDatabase,
    scheduler: &mut ScanScheduler,
    mut requests: Vec<ScanRequest>,
) {
    while let Some(req) = requests.pop() {
        let result = req.run();
        requests.extend(scheduler.apply_scan_completed(db, result));
    }
}

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
        uri: Uri::from_file_path(path).unwrap(),
        typ,
    }
}

fn workspace_state(workspace: &Path) -> WorldState {
    let mut state = WorldState::default();
    state
        .workspace
        .folders
        .push(AbsPathBuf::from_path_buf(workspace.to_path_buf()).unwrap());
    set_workspace_paths(&mut state, &[workspace.to_path_buf()]);
    state
}

#[test]
fn test_description_created_triggers_root_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    fs::write(tmp.path().join("pkg/R/a.R"), "x <- 1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    // No DESCRIPTION yet, so `a.R` registers as a workspace script.
    let root = state.db().workspace_roots().roots(state.db())[0];
    assert!(root.packages(state.db()).is_empty());

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

    let root = state.db().workspace_roots().roots(state.db())[0];
    assert_eq!(root.packages(state.db()).len(), 1);
    assert_eq!(root.packages(state.db())[0].name(state.db()), "pkg");
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

    let root = state.db().workspace_roots().roots(state.db())[0];
    assert_eq!(root.packages(state.db()).len(), 2);
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

    let root = state.db().workspace_roots().roots(state.db())[0];
    assert_eq!(root.scripts(state.db()).len(), 1);
    let file_path = FilePath::from_path_buf(path.clone()).unwrap();
    let file = state.db().file_by_path(&file_path).unwrap();
    assert_eq!(file.source_text(state.db()), "x <- 1\n");
}

#[test]
fn test_r_file_changed_for_editor_open_file_keeps_editor_contents() {
    // The editor is the source of truth for files it has open.
    // A disk-side `Changed` event that races against `didChange`
    // must not overwrite the editor's content.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.R");
    fs::write(&path, "disk_v1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let url = Url::from_file_path(&path).unwrap();
    let file_path = FilePath::from_url(&url);
    // Push the editor content into oak then register the buffer, the same as `didOpen`.
    let file = state
        .db_mut()
        .upsert_editor(file_path.clone(), "editor_v2\n".to_string());
    state.insert_open_file(url.to_uri().unwrap(), FilePath::from_url(&url), file, None);

    // Now disk-side `Changed` fires with stale disk content.
    fs::write(&path, "disk_v3\n").unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::CHANGED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let file = state.db().file_by_path(&file_path).unwrap();
    assert_eq!(file.source_text(state.db()), "editor_v2\n");
}

#[test]
fn test_r_file_deleted_routes_through_remove_file() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.R"), "x <- 1\n").unwrap();
    fs::write(tmp.path().join("b.R"), "y <- 2\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let path = tmp.path().join("a.R");
    let file_path = FilePath::from_path_buf(path.clone()).unwrap();
    fs::remove_file(&path).unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::DELETED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let root = state.db().workspace_roots().roots(state.db())[0];
    assert_eq!(root.scripts(state.db()).len(), 1);
    assert!(state.db().file_by_path(&file_path).is_none());
}

#[test]
fn test_r_file_changed_for_unopened_file_updates_contents() {
    // No editor buffer, so the watcher's disk content is authoritative
    // and should land in `file.source_text()`. Complements the open-file
    // test above.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.R");
    fs::write(&path, "v1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let file_path = FilePath::from_path_buf(path.clone()).unwrap();
    assert_eq!(
        state
            .db()
            .file_by_path(&file_path)
            .unwrap()
            .source_text(state.db()),
        "v1\n"
    );

    fs::write(&path, "v2\n").unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::CHANGED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    assert_eq!(
        state
            .db()
            .file_by_path(&file_path)
            .unwrap()
            .source_text(state.db()),
        "v2\n"
    );
}

#[test]
fn test_r_file_deleted_for_editor_open_file_keeps_buffer() {
    // Deleting the disk file must not hide an open buffer or replace the
    // editor contents used by analysis.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.R");
    fs::write(&path, "disk_v1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let url = Url::from_file_path(&path).unwrap();
    let file_path = FilePath::from_url(&url);
    let file = state
        .db_mut()
        .upsert_editor(file_path.clone(), "editor_v2\n".to_string());
    state.insert_open_file(url.to_uri().unwrap(), FilePath::from_url(&url), file, None);

    fs::remove_file(&path).unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&path, FileChangeType::DELETED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let file = state.db().file_by_path(&file_path).unwrap();
    assert_eq!(file.source_text(state.db()), "editor_v2\n");
}

#[test]
fn test_description_deleted_demotes_package_to_scripts() {
    // Inverse of `test_description_created_triggers_root_rescan`: a
    // DESCRIPTION removed mid-session triggers a root rescan and the
    // former package's R/ files surface as workspace scripts.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut state = workspace_state(tmp.path());

    let root = state.db().workspace_roots().roots(state.db())[0];
    assert_eq!(root.packages(state.db()).len(), 1);
    assert!(root.scripts(state.db()).is_empty());

    fs::remove_file(tmp.path().join("pkg/DESCRIPTION")).unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(
            &tmp.path().join("pkg/DESCRIPTION"),
            FileChangeType::DELETED,
        )],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let root = state.db().workspace_roots().roots(state.db())[0];
    assert!(root.packages(state.db()).is_empty());
    assert_eq!(root.scripts(state.db()).len(), 1);

    let file_path = FilePath::from_path_buf(tmp.path().join("pkg/R/a.R")).unwrap();
    let file = state.db().file_by_path(&file_path).unwrap();
    assert_eq!(file.package(state.db()), None);
}

fn folder(uri: &str) -> WorkspaceFolder {
    WorkspaceFolder {
        uri: uri.parse().unwrap(),
        name: String::new(),
    }
}

#[test]
fn test_effective_workspace_paths_reads_workspace_folders() {
    // Drive-letter URIs, because a folder has to survive as an `AbsPathBuf`.
    // `file:///a` has no drive so it isn't an absolute path on Windows, and
    // would be dropped there.
    let params = InitializeParams {
        workspace_folders: Some(vec![folder("file:///C:/a"), folder("file:///C:/b")]),
        ..Default::default()
    };
    let paths = effective_workspace_paths(&params);
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0].as_path().file_name(), Some("a"));
    assert_eq!(paths[1].as_path().file_name(), Some("b"));
}

// `root_uri` is deprecated in the protocol, but that's the point of these tests
#[allow(deprecated)]
#[test]
fn test_effective_workspace_paths_ignores_legacy_root_uri() {
    // We dropped the `root_uri` fallback, so a client sending only the
    // deprecated field gets single-file mode (empty), whether
    // `workspace_folders` is absent or an empty list.
    let absent = InitializeParams {
        workspace_folders: None,
        root_uri: Some("file:///legacy".parse().unwrap()),
        ..Default::default()
    };
    assert!(effective_workspace_paths(&absent).is_empty());

    let empty = InitializeParams {
        workspace_folders: Some(vec![]),
        root_uri: Some("file:///legacy".parse().unwrap()),
        ..Default::default()
    };
    assert!(effective_workspace_paths(&empty).is_empty());
}

#[allow(deprecated)]
#[test]
fn test_effective_workspace_paths_single_file_mode() {
    let params = InitializeParams {
        workspace_folders: None,
        root_uri: None,
        ..Default::default()
    };
    assert!(effective_workspace_paths(&params).is_empty());
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
        uri: Uri::from_file_path(path).unwrap(),
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
    assert_eq!(state.db().workspace_roots().roots(state.db()).len(), 1);

    let params = folders_change(vec![folder_for(second.path())], vec![]);
    did_change_workspace_folders(params, &mut state).unwrap();

    assert_eq!(state.workspace.folders.len(), 2);
    let roots = state.db().workspace_roots().roots(state.db()).clone();
    assert_eq!(roots.len(), 2);
    // Existing root stays first, new one appended.
    assert_eq!(roots[0].packages(state.db())[0].name(state.db()), "pkg1");
    assert_eq!(roots[1].packages(state.db())[0].name(state.db()), "pkg2");
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
        .push(AbsPathBuf::from_path_buf(first.path().to_path_buf()).unwrap());
    state
        .workspace
        .folders
        .push(AbsPathBuf::from_path_buf(second.path().to_path_buf()).unwrap());
    set_workspace_paths(&mut state, &[
        first.path().to_path_buf(),
        second.path().to_path_buf(),
    ]);
    assert_eq!(state.db().workspace_roots().roots(state.db()).len(), 2);

    let params = folders_change(vec![], vec![folder_for(first.path())]);
    did_change_workspace_folders(params, &mut state).unwrap();

    assert_eq!(state.workspace.folders.len(), 1);
    let roots = state.db().workspace_roots().roots(state.db()).clone();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].packages(state.db())[0].name(state.db()), "pkg2");
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
    assert_eq!(state.db().workspace_roots().roots(state.db()).len(), 1);
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
    let roots = state.db().workspace_roots().roots(state.db()).clone();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].packages(state.db())[0].name(state.db()), "pkg2");
}

#[test]
fn test_did_change_workspace_folders_preserves_open_buffer_across_churn() {
    // `did_change_workspace_folders()` must preserve an open buffer's identity,
    // contents, and analysis visibility across folder removal and re-addition.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut state = workspace_state(tmp.path());

    // Simulate `didOpen` on the package file with editor-side content.
    let r_path = tmp.path().join("pkg/R/a.R");
    let url = Url::from_file_path(&r_path).unwrap();
    let file_path = FilePath::from_url(&url);
    let file = state
        .db_mut()
        .upsert_editor(file_path.clone(), "editor <- 2\n".to_string());
    state.insert_open_file(url.to_uri().unwrap(), FilePath::from_url(&url), file, None);

    let file_before = state.db().file_by_path(&file_path).unwrap();

    // Removing the workspace folder keeps the open buffer visible through
    // `OrphanRoot`.
    let params = folders_change(vec![], vec![folder_for(tmp.path())]);
    did_change_workspace_folders(params, &mut state).unwrap();

    let after_remove = state.db().file_by_path(&file_path).unwrap();
    assert_eq!(file_before, after_remove);
    assert_eq!(after_remove.package(state.db()), None);
    assert!(state
        .db()
        .orphan_root()
        .files(state.db())
        .contains(&after_remove));
    assert_eq!(after_remove.source_text(state.db()), "editor <- 2\n");

    // Re-add the same folder. The file snaps back into pkg.files with
    // the same entity and the editor content carries over (the scan's
    // disk snapshot doesn't overwrite).
    let params = folders_change(vec![folder_for(tmp.path())], vec![]);
    did_change_workspace_folders(params, &mut state).unwrap();

    let after_readd = state.db().file_by_path(&file_path).unwrap();
    assert_eq!(file_before, after_readd);
    assert!(after_readd.package(state.db()).is_some());
    assert_eq!(after_readd.source_text(state.db()), "editor <- 2\n");
    // `upsert_root_file` cleaned the orphan reference.
    assert!(!state
        .db()
        .orphan_root()
        .files(state.db())
        .contains(&after_readd));
}

#[test]
fn test_did_close_releases_orphan_file() {
    // Closing a buffer after workspace eviction must remove its orphan
    // membership and override while retaining its identity. Without the
    // `close_editor()` call in `did_close()`, analysis would keep seeing the
    // closed buffer's unsaved contents.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut state = workspace_state(tmp.path());

    let r_path = tmp.path().join("pkg/R/a.R");
    let url = Url::from_file_path(&r_path).unwrap();
    let file_path = FilePath::from_url(&url);

    // Simulate `didOpen` via state mutation (matches the rest of the file's
    // pattern).
    let file = state
        .db_mut()
        .upsert_editor(file_path.clone(), "edited\n".to_string());
    state.insert_open_file(url.to_uri().unwrap(), FilePath::from_url(&url), file, None);

    did_change_workspace_folders(
        folders_change(vec![], vec![folder_for(tmp.path())]),
        &mut state,
    )
    .unwrap();
    let file = state.db().file_by_path(&file_path).unwrap();
    assert!(state.db().orphan_root().files(state.db()).contains(&file));

    // Init the aux channel here, after the workspace-folders churn: the
    // handler wrapper resets the channel each call (it stands up its own to
    // satisfy `send_auxiliary`), so grab the receiver only once that's done.
    let mut aux_rx = init_aux_for_test();

    let params = DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier {
            uri: url.to_uri().unwrap(),
        },
    };
    did_close(params, &mut state).unwrap();

    assert!(!state.db().orphan_root().files(state.db()).contains(&file));
    assert_eq!(state.db().file_by_path(&file_path), None);
    assert_eq!(state.db_mut().interned_file(&file_path), Some(file));
    assert_eq!(file.source_text_override(state.db()), &None);

    // did_close() clears diagnostics for the closed file.
    let event = aux_rx.try_recv().unwrap();
    let AuxiliaryEvent::PublishDiagnostics(publication) = event else {
        panic!("expected a PublishDiagnostics event");
    };
    assert_eq!(publication.path, FilePath::from_url(&url));
    assert!(publication.diagnostics.is_empty());
}

fn rescan_without_open_file(state: &mut WorldState, workspace: &Path, r_path: &Path) -> Uri {
    let uri = Uri::from_file_path(r_path).unwrap();
    let params = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "r".to_string(),
            version: 1,
            text: "x <- 1\n".to_string(),
        },
    };
    did_open(params, state).unwrap();

    fs::remove_file(r_path).unwrap();
    let rprofile = workspace.join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&rprofile, FileChangeType::CREATED)],
    };
    did_change_watched_files(params, state).unwrap();

    // Checked before any `did_change()`, which would restore visibility by
    // itself through `upsert_editor()` and hide a missing rescue.
    let path = uri.to_document_path().unwrap();
    let file = state.open_file(&path).unwrap().file();
    assert_eq!(state.db().file_by_path(&path), Some(file));

    uri
}

/// Each edit goes through `did_change()` and must keep writing to the held
/// `File`. The second edit must use the first edit's result as its input.
/// Applying it to the original text would insert `z` after the space rather
/// than after `yy`.
fn assert_incremental_edits_apply(state: &mut WorldState, uri: &Uri) {
    let mut lsp_state = test_lsp_state();
    let edits = [
        (
            Range::new(Position::new(0, 0), Position::new(0, 1)),
            "yy",
            "yy <- 1\n",
        ),
        (
            Range::new(Position::new(0, 2), Position::new(0, 2)),
            "z",
            "yyz <- 1\n",
        ),
    ];
    for (version, (range, text, expected)) in (2..).zip(edits) {
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(range),
                range_length: None,
                text: text.to_string(),
            }],
        };
        did_change(params, &mut lsp_state, state).unwrap();

        let path = uri.to_document_path().unwrap();
        let file = state.open_file(&path).unwrap().file();
        assert_eq!(state.db().file_by_path(&path), Some(file));
        assert_eq!(file.source_text(state.db()), expected);
    }
}

/// A rescan must keep an open buffer's `File` visible to analysis even when its
/// file is missing on disk. Otherwise `file_by_path()` misses the entity that
/// `did_change()` keeps writing to.
#[test]
fn test_rescan_keeps_open_script_missing_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let r_path = tmp.path().join("a.R");
    fs::write(&r_path, "x <- 1\n").unwrap();
    let mut state = workspace_state(tmp.path());

    let uri = rescan_without_open_file(&mut state, tmp.path(), &r_path);

    let root = state.db().workspace_roots().roots(state.db())[0];
    assert!(root.scripts(state.db()).is_empty());
    assert_incremental_edits_apply(&mut state, &uri);
}

#[test]
fn test_rescan_keeps_open_package_file_missing_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[
        ("a.R", "x <- 1\n"),
        ("b.R", "y <- 1\n"),
    ]);
    let mut state = workspace_state(tmp.path());

    let r_path = tmp.path().join("pkg/R/a.R");
    let uri = rescan_without_open_file(&mut state, tmp.path(), &r_path);

    let path = uri.to_document_path().unwrap();
    let file = state.open_file(&path).unwrap().file();
    let root = state.db().workspace_roots().roots(state.db())[0];
    let pkg = root.packages(state.db())[0];
    assert!(!pkg.files(state.db()).contains(&file));
    assert_eq!(file.package(state.db()), None);
    assert_incremental_edits_apply(&mut state, &uri);
}

#[test]
fn test_library_watchers_watch_direct_entries_of_each_library_root() {
    let lib1 = tempfile::tempdir().unwrap();
    let lib2 = tempfile::tempdir().unwrap();
    let mut state = WorldState::default();
    state
        .db_mut()
        .set_library_paths(&[lib1.path().to_path_buf(), lib2.path().to_path_buf()]);

    let watcher = |lib: &Path| FileSystemWatcher {
        glob_pattern: GlobPattern::Relative(RelativePattern {
            base_uri: OneOf::Right(Uri::from_file_path(lib).unwrap()),
            pattern: String::from("*"),
        }),
        kind: None,
    };
    assert_eq!(library_watchers(state.db()), vec![
        watcher(lib1.path()),
        watcher(lib2.path()),
    ]);
}

#[test]
fn test_library_entry_event_makes_installed_package_resolvable() {
    let lib = tempfile::tempdir().unwrap();
    let mut state = WorldState::default();
    state
        .db_mut()
        .set_library_paths(&[lib.path().to_path_buf()]);
    assert!(state.db().package_by_name("pkg").is_none());

    write_package(&lib.path().join("pkg"), "pkg", &[]);
    oak_package_metadata::tests::mark_installed(&lib.path().join("pkg"));
    let params = DidChangeWatchedFilesParams {
        changes: vec![event(&lib.path().join("pkg"), FileChangeType::CREATED)],
    };
    did_change_watched_files(params, &mut state).unwrap();

    let pkg = state.db().package_by_name("pkg").unwrap();
    let root = state.db().library_roots().roots(state.db())[0];
    assert_eq!(root.packages(state.db()), &vec![pkg]);
}

/// A console-inputs push carries no oak write, but diagnostics read the console
/// scopes it updates. The handler advances the oak revision so the main loop's
/// central refresh (and its snapshot barrier) fires. Pin that: reverting to a
/// direct refresh that doesn't bump the revision would silently stop the
/// central refresh and let a stale-scope pass publish last.
#[test]
fn test_console_inputs_advance_revision() {
    let mut state = WorldState::default();
    let before = salsa::plumbing::current_revision(state.db());

    did_change_console_inputs(
        ConsoleInputs {
            console_scopes: vec![vec!["foo".to_string()]],
        },
        &mut state,
    )
    .unwrap();

    let after = salsa::plumbing::current_revision(state.db());
    assert_ne!(before, after);
}
