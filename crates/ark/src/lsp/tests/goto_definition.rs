use std::collections::HashMap;
use std::sync::Arc;

use aether_path::FilePath;
use assert_matches::assert_matches;
use oak_db::Db;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::GotoDefinitionParams;
use tower_lsp_server::ls_types::GotoDefinitionResponse;
use tower_lsp_server::ls_types::Uri;
use url::Url;

use super::source_handler::TestBehavior;
use super::source_handler::TestSourceHandler;
use super::utils::did_change_workspace_folders;
use super::utils::insert_file;
use super::utils::make_state;
use super::utils::range;
use super::utils::source_scheduler_for_test;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use super::utils::NamespaceWriter;
use crate::lsp::goto_definition::goto_definition;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UrlExt;
use crate::lsp::util::test_path;

fn make_params(uri: &Uri, line: u32, character: u32) -> GotoDefinitionParams {
    GotoDefinitionParams {
        text_document_position_params: lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            position: lsp_types::Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

/// A state with several open files, each mirrored into `oak` like `did_open`
/// does, so `source()` targets resolve through `file_by_path`.
fn make_state_with(files: &[(&str, &str)]) -> WorldState {
    let mut state = WorldState::default();
    for (wire, code) in files {
        insert_file(&mut state, wire, code);
    }
    state
}

#[test]
fn test_goto_definition() {
    let (state, uri) = make_state(test_path("test.R").as_str(), "foo <- 42\nprint(foo)\n");

    let params = make_params(&uri, 1, 6);

    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );
}

#[test]
fn test_goto_definition_prefers_local_symbol() {
    let (state, uri) = make_state(test_path("file.R").as_str(), "foo <- 1\nfoo\n");

    let params = make_params(&uri, 1, 0);

    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_uri, uri);
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );
}

#[test]
fn test_target_uri_is_verbatim() {
    // `FilePath` normalisation would collapse the doubled slash, decode and
    // re-encode the percent-escaped drive colon with different casing, and
    // leave the encoded brackets raw, so a `Url` round-trip changes all three
    // wire strings below. The target URI is only correct if it comes from the
    // buffer's verbatim URI.
    for wire in [
        "file:///C:/proj//file.R",
        "file:///c%3A/proj/file.R",
        "file:///C:/proj/f%5B1%5D.R",
    ] {
        let (state, uri) = make_state(wire, "foo <- 1\nfoo\n");

        let params = make_params(&uri, 1, 0);

        assert_matches!(
            goto_definition(params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri.as_str(), wire);
            }
        );
        // Confirm the round-trip really would have differed, so the check
        // above bites.
        let url = Url::parse(wire).unwrap();
        assert_ne!(FilePath::from_url(&url).to_url(), url);
    }
}

#[test]
fn test_unbound_identifier_returns_none() {
    // A free identifier with no reachable binding returns `None`, matching how
    // rust-analyzer and ty handle the same case.
    let (state, uri) = make_state(test_path("file.R").as_str(), "foo\n");

    let params = make_params(&uri, 0, 0);
    assert_eq!(goto_definition(params, &state).unwrap(), None);
}

#[test]
fn test_cursor_on_operator_returns_none() {
    // Cursor on `<-`, not on an identifier use: nothing to resolve.
    let (state, uri) = make_state(test_path("file.R").as_str(), "foo <- 1\n");

    // Cursor on the `<` of `<-` at column 4.
    let params = make_params(&uri, 0, 4);
    assert_eq!(goto_definition(params, &state).unwrap(), None);
}

#[test]
fn test_unlinked_cross_file_returns_none() {
    // `foo` is defined in another open file, but this file doesn't `source()`
    // it, so R semantics can't reach it. goto-def is precise: it returns `None`
    // rather than guessing by name across the workspace like the legacy ark handler.
    let uses_wire = test_path("uses.R");
    let uses_uri: Uri = uses_wire.as_str().parse().unwrap();
    let state = make_state_with(&[
        (uses_wire.as_str(), "foo\n"),
        (test_path("defs.R").as_str(), "foo <- function() 1\n"),
    ]);

    let params = make_params(&uses_uri, 0, 0);
    assert_eq!(goto_definition(params, &state).unwrap(), None);
}

#[test]
fn test_resolves_across_source_directive() {
    // `script.R` sources `helpers.R`; goto-def on the forwarded `helper` use
    // lands in `helpers.R`. Exercises the cross-file branch of
    // `definition_to_link` (the target file's own line index + URL). The
    // resolution itself is covered exhaustively by `oak_db`'s `file_resolve_at`
    // tests; this checks the goto-def wiring on top of it.
    let script_wire = test_path("script.R");
    let helpers_wire = test_path("helpers.R");
    let helpers_uri: Uri = helpers_wire.as_str().parse().unwrap();
    let state = make_state_with(&[
        (script_wire.as_str(), "source(\"helpers.R\")\nhelper\n"),
        (helpers_wire.as_str(), "helper <- function() 1\n"),
    ]);

    let script_uri: Uri = script_wire.as_str().parse().unwrap();
    let params = make_params(&script_uri, 1, 0);
    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_uri, helpers_uri);
            assert_eq!(links[0].target_range, range((0, 0), (0, 6)));
        }
    );
}

#[test]
fn test_local_def_shadows_sourced() {
    // A local `<-` after a `source()` shadows the sourced binding, so the use
    // resolves to the local def (in this file), not the sourced one. The link
    // range must point at the local def.
    let script_wire = test_path("script.R");
    let helpers_wire = test_path("helpers.R");
    let script_uri: Uri = script_wire.as_str().parse().unwrap();
    let state = make_state_with(&[
        (
            script_wire.as_str(),
            "source(\"helpers.R\")\nfoo <- 1\nfoo\n",
        ),
        (helpers_wire.as_str(), "foo <- function() 2\n"),
    ]);

    let params = make_params(&script_uri, 2, 0);
    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_uri, script_uri);
            assert_eq!(links[0].target_range, range((1, 0), (1, 3)));
        }
    );
}

#[test]
fn test_sourced_file_with_repeated_def_offers_both() {
    // When the sourced file binds the same name on both arms of a top-level
    // `if`/`else`, either arm could run, so goto-def offers both candidate
    // definitions, in definition order. Ranges are in the target file's
    // coordinates.
    let script_wire = test_path("script.R");
    let helpers_wire = test_path("helpers.R");
    let helpers_uri: Uri = helpers_wire.as_str().parse().unwrap();
    let state = make_state_with(&[
        (script_wire.as_str(), "source(\"helpers.R\")\nfn\n"),
        (helpers_wire.as_str(), "if (cond) fn <- 1 else fn <- 2\n"),
    ]);

    let script_uri: Uri = script_wire.as_str().parse().unwrap();
    let params = make_params(&script_uri, 1, 0);
    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 2);
            assert_eq!(links[0].target_uri, helpers_uri);
            assert_eq!(links[0].target_range, range((0, 10), (0, 12)));
            assert_eq!(links[1].target_uri, helpers_uri);
            assert_eq!(links[1].target_range, range((0, 23), (0, 25)));
        }
    );
}

#[test]
fn test_sourced_file_with_sequential_redef_offers_runtime_winner() {
    // When the sourced file binds the same name twice in sequence, the second
    // overwrites the first, so only the last binding is in effect when
    // `source()` finishes. goto-def offers just that one.
    let script_wire = test_path("script.R");
    let helpers_wire = test_path("helpers.R");
    let helpers_uri: Uri = helpers_wire.as_str().parse().unwrap();
    let state = make_state_with(&[
        (script_wire.as_str(), "source(\"helpers.R\")\nfn\n"),
        (
            helpers_wire.as_str(),
            "fn <- function() 'first'\nfn <- function() 'second'\n",
        ),
    ]);

    let script_uri: Uri = script_wire.as_str().parse().unwrap();
    let params = make_params(&script_uri, 1, 0);
    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, helpers_uri);
            assert_eq!(links[0].target_range, range((1, 0), (1, 2)));
        }
    );
}

/// Goto-def from a bare `foo()` through to `foopkg::foo()`'s definition via the
/// workspace package's `import(foopkg)`
#[tokio::test]
async fn test_goto_definition_resolves_unqualified_import_into_package() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("foopkg"),
        TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
    )])));

    // Set up the library that our "installed" package lives in
    let library = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("foopkg")
        .version("0.0.0")
        .built("dummy")
        .write(&library.path().join("foopkg"));
    NamespaceWriter::new()
        .export("foo")
        .write(&library.path().join("foopkg"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[library.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db, true),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler),
        ),
    );

    // The workspace folder is itself the package that imports `foo`.
    let workspace = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("mypackage")
        .version("0.0.0")
        .imports(&["foopkg"])
        .write(workspace.path());
    NamespaceWriter::new()
        .import("foopkg")
        .write(workspace.path());
    let use_path = workspace.path().join("R").join("use.R");
    let use_uri: Uri = Url::from_file_path(&use_path).unwrap().to_uri().unwrap();
    write_sources(&workspace.path().join("R"), &[("use.R", "foo()\n")]);

    // Open the workspace folder, triggering a workspace scan
    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    let world = state.world();
    let foo_file = world.db.package_by_name("foopkg").unwrap().files(&world.db)[0];

    assert_matches!(
        goto_definition(make_params(&use_uri, 0, 0), world).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, world.wire_uri(foo_file).unwrap());
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );
}

/// Goto-def from a bare `bar()` through to `barpkg::bar()`'s definition via the
/// workspace package's `importFrom(barpkg, bar)`
#[tokio::test]
async fn test_goto_definition_resolves_unqualified_import_from_into_package() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("barpkg"),
        TestBehavior::Success(vec![("bar.R", "bar <- function() 2\n")]),
    )])));

    // Set up the library that our "installed" package lives in
    let library = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("barpkg")
        .version("0.0.0")
        .built("dummy")
        .write(&library.path().join("barpkg"));
    NamespaceWriter::new()
        .export("bar")
        .write(&library.path().join("barpkg"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[library.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db, true),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler),
        ),
    );

    // The workspace folder is itself the package that imports `bar`.
    let workspace = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("mypackage")
        .version("0.0.0")
        .imports(&["barpkg"])
        .write(workspace.path());
    NamespaceWriter::new()
        .import_from("barpkg", "bar")
        .write(workspace.path());
    let use_path = workspace.path().join("R").join("use.R");
    let use_uri: Uri = Url::from_file_path(&use_path).unwrap().to_uri().unwrap();
    write_sources(&workspace.path().join("R"), &[("use.R", "bar()\n")]);

    // Open the workspace folder, triggering a workspace scan
    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    let world = state.world();
    let bar_file = world.db.package_by_name("barpkg").unwrap().files(&world.db)[0];

    assert_matches!(
        goto_definition(make_params(&use_uri, 0, 0), world).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, world.wire_uri(bar_file).unwrap());
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );
}

/// Goto-def through both `pkg::foo()` (exported) and `pkg:::bar()` (internal,
/// unexported). The triple colon reaches bindings the double colon couldn't.
#[tokio::test]
async fn test_goto_definition_resolves_namespace_accesses() {
    let _aux = init_aux_for_test();

    // `pkg` exports `foo` but not `bar`.
    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("pkg"),
        TestBehavior::Success(vec![(
            "test.R",
            "foo <- function() 1\nbar <- function() 1\n",
        )]),
    )])));

    // Set up the library that our "installed" package lives in
    let library = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("pkg")
        .version("0.0.0")
        .built("dummy")
        .write(&library.path().join("pkg"));
    NamespaceWriter::new()
        .export("foo")
        .write(&library.path().join("pkg"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[library.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db, true),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler),
        ),
    );

    // Workspace usage pkg functions
    let workspace = tempfile::tempdir().unwrap();
    let use_path = workspace.path().join("R").join("use.R");
    let use_uri: Uri = Url::from_file_path(&use_path).unwrap().to_uri().unwrap();
    write_sources(&workspace.path().join("R"), &[(
        "use.R",
        "pkg::foo()\npkg:::bar()\npkg:::foo()\npkg::bar()\n",
    )]);

    // Open the workspace folder, triggering a workspace scan
    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    let world = state.world();
    let file = world.db.package_by_name("pkg").unwrap().files(&world.db)[0];

    // `pkg::f<@>oo()`, exported `foo` reached through `::`
    assert_matches!(
        goto_definition(make_params(&use_uri, 0, 6), world).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, world.wire_uri(file).unwrap());
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );

    // `pkg:::b<@>ar()`, internal `bar` reached through `:::`
    assert_matches!(
        goto_definition(make_params(&use_uri, 1, 7), world).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, world.wire_uri(file).unwrap());
            assert_eq!(links[0].target_range, range((1, 0), (1, 3)));
        }
    );

    // `pkg:::f<@>oo()`, `:::` also reaches an exported binding
    assert_matches!(
        goto_definition(make_params(&use_uri, 2, 7), world).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, world.wire_uri(file).unwrap());
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );

    // `pkg::b<@>ar()`, `::` can't reach the unexported `bar`
    assert_eq!(
        goto_definition(make_params(&use_uri, 3, 6), world).unwrap(),
        None
    );
}
