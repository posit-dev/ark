//
// utils.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

use std::collections::HashSet;
use std::path::Path;

use aether_path::FilePath;
use oak_db::OakDatabase;
use oak_db::SourceDb;
use oak_scan::DbScan;
use oak_scan::ScanScheduler;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionTextEdit;
use tree_sitter::Point;
use url::Url;

use crate::fixtures::point_from_cursor;
use crate::lsp::completions::finish;
use crate::lsp::completions::provide::StaticCompletions;
use crate::lsp::completions::r_completions;
use crate::lsp::completions::request::RCompletionReplies;
use crate::lsp::completions::request::RCompletionRequest;
use crate::lsp::completions::sources::utils::has_priority_prefix;
use crate::lsp::completions::static_completions;
use crate::lsp::document_context::open_test_document;
use crate::lsp::document_context::test_document_context;
use crate::lsp::state::WorldState;

/// Runs both completion passes. Requires the R thread.
pub(crate) fn get_completions_at_cursor(cursor_text: &str) -> anyhow::Result<Vec<CompletionItem>> {
    let (completions, request) = static_completions_at_cursor(cursor_text)?;
    let replies = r_completions(request);
    Ok(finish(completions, replies))
}

/// Runs only the static pass.
pub(crate) fn static_completions_at_cursor(
    cursor_text: &str,
) -> anyhow::Result<(StaticCompletions, RCompletionRequest)> {
    let (text, point) = point_from_cursor(cursor_text);
    static_completions_at(&text, point)
}

pub(crate) fn static_completions_at(
    text: &str,
    point: Point,
) -> anyhow::Result<(StaticCompletions, RCompletionRequest)> {
    static_completions_in(&mut WorldState::default(), text, point)
}

/// Runs only the static pass against `state`, with the document opened in
/// `state`'s database.
pub(crate) fn static_completions_with_state(
    cursor_text: &str,
    state: &mut WorldState,
) -> (StaticCompletions, RCompletionRequest) {
    let (text, point) = point_from_cursor(cursor_text);
    static_completions_in(state, &text, point).unwrap()
}

/// Return static items with no session contributions, as when R is busy.
pub(crate) fn static_items_with_state(
    cursor_text: &str,
    state: &mut WorldState,
) -> Vec<CompletionItem> {
    let (completions, _) = static_completions_with_state(cursor_text, state);
    finish(completions, RCompletionReplies::default())
}

/// Runs only the static pass, with `pkg` installed in the library and its
/// fetched sources defining the exported functions `adist()` and `aspell()`.
pub(crate) fn static_items_with_test_package(cursor_text: &str) -> Vec<CompletionItem> {
    let library = tempfile::TempDir::new().unwrap();
    let mut state = write_library_package(
        library.path(),
        "pkg",
        "export(adist, aspell)\n",
        Some(&[(
            "pkg.R",
            "adist <- function(x, y = NULL) NULL\naspell <- function(files) NULL\n",
        )]),
    );
    static_items_with_state(cursor_text, &mut state)
}

/// A state whose database has scanned R's own library, so the base and
/// recommended packages are known from their `DESCRIPTION` and `NAMESPACE`
/// files. Their sources aren't fetched.
pub(crate) fn state_with_r_library() -> WorldState {
    let r_home = harp::command::r_home_setup().unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[r_home.join("library")]);
    WorldState::with_db(db)
}

/// Install `name` in the state's only library, with the supplied `NAMESPACE`.
/// If supplied, ingest `sources` as fetched package sources.
pub(crate) fn write_library_package(
    library: &Path,
    name: &str,
    namespace: &str,
    sources: Option<&[(&str, &str)]>,
) -> WorldState {
    let package_dir = library.join(name);
    write_package(&package_dir, name, namespace);

    let mut db = OakDatabase::new();
    db.set_library_paths(&[library.to_path_buf()]);

    if let Some(sources) = sources {
        let sources_dir = library.join(format!("{name}-sources"));
        write_files(&sources_dir, sources);
        let package = db.package_by_name(name).unwrap();
        db.set_package_sources(package, &sources_dir);
    }

    WorldState::with_db(db)
}

/// A state whose database has `workspace` as its only workspace root, holding
/// the package `name` with `namespace` as its `NAMESPACE` and `sources` under
/// `R/`.
pub(crate) fn state_with_workspace_package(
    workspace: &Path,
    name: &str,
    namespace: &str,
    sources: &[(&str, &str)],
) -> WorldState {
    write_package(workspace, name, namespace);
    write_files(&workspace.join("R"), sources);

    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let editor_owned = HashSet::new();
    let mut requests =
        scheduler.set_workspace_paths(&mut db, &[workspace.to_path_buf()], &editor_owned);
    while let Some(request) = requests.pop() {
        let completed = request.run();
        requests.extend(scheduler.apply_scan_completed(&mut db, completed, &editor_owned));
    }

    WorldState::with_db(db)
}

fn write_package(dir: &Path, name: &str, namespace: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 1.0.0\n"),
    )
    .unwrap();
    std::fs::write(dir.join("NAMESPACE"), namespace).unwrap();
}

fn write_files(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (basename, contents) in files {
        std::fs::write(dir.join(basename), contents).unwrap();
    }
}

/// Runs only the static pass, with `workspace_text` as the contents of a
/// workspace file.
pub(crate) fn static_completions_with_workspace(
    cursor_text: &str,
    workspace_text: &str,
) -> (StaticCompletions, RCompletionRequest) {
    let mut db = OakDatabase::new();
    let path = FilePath::from_url(&Url::parse("file:///workspace.R").unwrap());
    db.upsert_editor(path, workspace_text.to_string());

    let (text, point) = point_from_cursor(cursor_text);
    static_completions_in(&mut WorldState::with_db(db), &text, point).unwrap()
}

fn static_completions_in(
    state: &mut WorldState,
    text: &str,
    point: Point,
) -> anyhow::Result<(StaticCompletions, RCompletionRequest)> {
    let file = open_test_document(state, text);
    let document_context = test_document_context(state, file, point, None);

    match static_completions(&document_context, file, state) {
        Ok(completions) => Ok(completions),
        Err(err) => Err(anyhow::anyhow!("Failed to get completions: {err}")),
    }
}

/// Returns the session request built by the static pass.
pub(crate) fn request_at_cursor(cursor_text: &str) -> RCompletionRequest {
    let (_, request) = static_completions_at_cursor(cursor_text).unwrap();
    request
}

pub(crate) fn find_completion_by_label<'a>(
    completions: &'a [lsp_types::CompletionItem],
    label: &str,
) -> Option<&'a lsp_types::CompletionItem> {
    completions.iter().find(|c| c.label == label)
}

pub(crate) fn assert_text_edit(item: &lsp_types::CompletionItem, expected_text: &str) {
    assert!(item.text_edit.is_some());
    assert!(item.insert_text.is_none());

    match item.text_edit.as_ref().unwrap() {
        CompletionTextEdit::Edit(edit) => {
            assert_eq!(
                edit.new_text, expected_text,
                "Text edit should replace with '{expected_text}'"
            );
        },
        _ => panic!("Unexpected TextEdit variant"),
    }
}

pub(crate) fn assert_has_parameter_hints(item: &lsp_types::CompletionItem) {
    match &item.command {
        Some(command) => assert_eq!(command.command, "editor.action.triggerParameterHints"),
        None => panic!("CompletionItem is missing parameter hints command"),
    }
}

pub(crate) fn assert_no_command(item: &lsp_types::CompletionItem) {
    assert!(
        item.command.is_none(),
        "CompletionItem should not have an associated command"
    );
}

pub(crate) fn assert_sort_text_has_priority_prefix(item: &lsp_types::CompletionItem) {
    assert!(item.sort_text.is_some());
    let sort_text = item.sort_text.as_ref().unwrap();
    assert!(has_priority_prefix(sort_text));
}
