//! `base` combines archive sources and independently generated primitive stubs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aether_path::FilePath;
use oak_db::BindingKind;
use oak_db::Name;
use oak_db::NamespaceVisibility;
use oak_db::OakDatabase;
use oak_db::SourceDb;
use oak_ide::NamespaceItem;
use oak_scan::DbScan;
use oak_sidecar::primitives::PrimitivesCache;
use serde_json::json;

use super::sidecar_handler::TestSidecarHandler;
use super::source_handler::TestBehavior;
use super::source_handler::TestSourceHandler;
use super::utils::did_change_workspace_folders;
use super::utils::initialize;
use super::utils::initialized;
use super::utils::source_scheduler_for_test;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use super::utils::TestClient;
use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_SETTING;
use crate::lsp::harness::TestHandlers;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sidecar::SidecarScheduler;
use crate::lsp::sources::SourceScheduler;
use crate::panic;

/// Run the primitives script against the test R and check the stub file against the
/// primitives R itself reports.
#[test]
fn test_primitive_stubs_describe_every_primitive() {
    let r_home = harp::command::r_home_setup().unwrap();
    let r = oak_r_process::r_executable(&r_home).unwrap();

    let output = oak_r_process::run_text(
        &r,
        r#"
base <- packageDescription("base")
cat(base$Version, base$Built, sep = "\n")
names <- ls(baseenv(), all.names = TRUE)
is_primitive <- vapply(
    names,
    function(name) is.primitive(get(name, envir = baseenv())),
    logical(1)
)
cat(names[is_primitive], sep = "\n")
"#,
        &oak_r_process::RunOptions::default(),
    )
    .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();
    let version = lines.next().unwrap();
    let built = lines.next().unwrap();
    let mut expected: Vec<&str> = lines.collect();
    expected.sort();

    let cache = tempfile::tempdir().unwrap();
    let primitives = PrimitivesCache::open_in(cache.path().to_path_buf(), r).unwrap();
    let directory = primitives.insert(version, built).unwrap();

    let stub_paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "R"))
        .collect();
    assert_eq!(stub_paths.len(), 1);
    let text = std::fs::read_to_string(&stub_paths[0]).unwrap();

    let parse = aether_parser::parse(&text, Default::default());
    assert!(parse.diagnostics().is_empty());
    assert_eq!(
        parse.tree().statements().into_iter().count(),
        expected.len()
    );

    let mut db = OakDatabase::new();
    let file = db.upsert_editor(
        FilePath::from_path_buf(stub_paths[0].clone()).unwrap(),
        text.clone(),
    );
    let bindings = file.bindings(&db);
    let mut names: Vec<&str> = bindings.iter().map(|(name, _)| name).collect();
    names.sort();
    assert_eq!(names, expected);
    assert!(bindings
        .iter()
        .all(|(_, kind)| kind == BindingKind::Function));

    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.contains(&"sum <- function(..., na.rm = FALSE) NULL"));
    assert!(lines.contains(&"`[` <- .Primitive(\"[\")"));

    assert_eq!(primitives.get(version, built), Some(directory));
}

const STUBS: &str = "sum <- function(..., na.rm = FALSE) NULL\n`[` <- .Primitive(\"[\")\n";

/// A library holding an installed `base` without sources.
fn library_with_base() -> (tempfile::TempDir, OakDatabase) {
    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("base")
        .version("4.5.2")
        .built("dummy")
        .field("Priority", "base")
        .write_installed(&lib.path().join("base"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    (lib, db)
}

fn sidecar_with_stubs() -> Arc<TestSidecarHandler> {
    Arc::new(TestSidecarHandler::new(vec![("primitives.R", STUBS)]))
}

fn assert_base_is_stubs(state: &GlobalState<TestHandlers>) {
    let _boundary = panic::test_boundary();
    let db = state.world().db();
    let base = db.package_by_name("base").unwrap();

    let files = base.files(db).to_vec();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].source_text(db), STUBS);
    assert!(base.scripts(db).is_empty());

    let definitions = base.resolve(db, Name::new(db, "sum"), NamespaceVisibility::Exported);
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].file(db), files[0]);

    let completions =
        oak_ide::namespace_completions(db, "base", NamespaceVisibility::Exported).unwrap();
    assert_eq!(completions.items, vec![
        NamespaceItem {
            name: String::from("["),
            kind: BindingKind::Function,
        },
        NamespaceItem {
            name: String::from("sum"),
            kind: BindingKind::Function,
        },
    ]);
}

/// The sidecar job doesn't depend on `oak.sourceFetching.enabled`, so the stubs land
/// while no source request is made.
#[tokio::test]
async fn test_primitive_stubs_land_with_fetching_off() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("base"),
        TestBehavior::Failure,
    )])));
    let sidecar = sidecar_with_stubs();

    let (_lib, db) = library_with_base();
    let client = TestClient::new(&[(OAK_SOURCE_FETCHING_ENABLED_SETTING, json!(false))]).await;
    let mut state = GlobalState::from_parts(
        TestHandlers,
        client.client(),
        world_with_source_fetching(db),
        LspState::new()
            .with_source_scheduler(SourceScheduler::new(Some(handler.clone())))
            .with_sidecar_scheduler(SidecarScheduler::new(Some(sidecar.clone()))),
    );

    let workspace = tempfile::tempdir().unwrap();
    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;
    state.handle_event_to_quiescence(initialized()).await;

    assert!(handler.calls().lock().unwrap().is_empty());
    assert_eq!(sidecar.calls().lock().unwrap().len(), 1);
    assert_base_is_stubs(&state);
}

/// If the archive is unavailable, primitive stubs still provide `base::`
/// resolution and completions.
#[tokio::test]
async fn test_primitive_stubs_land_when_base_fetch_fails() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("base"),
        TestBehavior::Failure,
    )])));

    let (_lib, db) = library_with_base();
    let mut state = GlobalState::from_parts(
        TestHandlers,
        test_client(),
        world_with_source_fetching(db),
        LspState::new()
            .with_source_scheduler(source_scheduler_for_test(handler.clone()))
            .with_sidecar_scheduler(SidecarScheduler::new(Some(sidecar_with_stubs()))),
    );

    let workspace = tempfile::tempdir().unwrap();
    write_sources(workspace.path(), &[("use.R", "base::sum(1)\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    let names: Vec<String> = handler
        .calls()
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.name().to_string())
        .collect();
    assert!(names.contains(&String::from("base")));
    assert_base_is_stubs(&state);
}

/// Archive sources precede primitive stubs in `base`'s file list, regardless
/// of which job finishes first.
#[tokio::test]
async fn test_base_archive_and_stubs_both_land() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("base"),
        TestBehavior::Success(vec![("paste.R", "paste <- function(...) NULL\n")]),
    )])));

    let (_lib, db) = library_with_base();
    let mut state = GlobalState::from_parts(
        TestHandlers,
        test_client(),
        world_with_source_fetching(db),
        LspState::new()
            .with_source_scheduler(source_scheduler_for_test(handler))
            .with_sidecar_scheduler(SidecarScheduler::new(Some(sidecar_with_stubs()))),
    );

    let workspace = tempfile::tempdir().unwrap();
    write_sources(workspace.path(), &[("use.R", "base::sum(1)\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    // A guard can't be held across `.await`, so it's entered after the last one
    let _boundary = panic::test_boundary();
    let db = state.world().db();
    let base = db.package_by_name("base").unwrap();
    let texts: Vec<&str> = base
        .files(db)
        .iter()
        .map(|file| file.source_text(db).as_str())
        .collect();
    assert_eq!(texts, vec!["paste <- function(...) NULL\n", STUBS]);
}
