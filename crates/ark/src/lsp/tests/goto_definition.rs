use assert_matches::assert_matches;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use url::Url;

use super::utils::insert_file;
use super::utils::make_state;
use super::utils::range;
use crate::lsp::document::Document;
use crate::lsp::goto_definition::goto_definition;
use crate::lsp::state::WorldState;
use crate::lsp::util::test_path;

fn make_params(uri: lsp_types::Url, line: u32, character: u32) -> GotoDefinitionParams {
    GotoDefinitionParams {
        text_document_position_params: lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            position: lsp_types::Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

/// A state with several open files, each mirrored into `oak` like `did_open`
/// does, so `source()` targets resolve through `file_by_url`.
fn make_state_with(files: &[(&Url, &str)]) -> WorldState {
    let mut state = WorldState::default();
    for (uri, code) in files {
        insert_file(&mut state, uri, &Document::new(code, None));
    }
    state
}

#[test]
fn test_goto_definition() {
    let uri = test_path("test.R");
    let state = make_state(&uri, &Document::new("foo <- 42\nprint(foo)\n", None));

    let params = make_params(uri, 1, 6);

    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );
}

#[test]
fn test_goto_definition_prefers_local_symbol() {
    let uri = test_path("file.R");
    let state = make_state(&uri, &Document::new("foo <- 1\nfoo\n", None));

    let params = make_params(uri.clone(), 1, 0);

    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_uri, uri);
            assert_eq!(links[0].target_range, range((0, 0), (0, 3)));
        }
    );
}

#[test]
fn test_unbound_identifier_returns_none() {
    // A free identifier with no reachable binding returns `None`, matching how
    // rust-analyzer and ty handle the same case.
    let uri = test_path("file.R");
    let state = make_state(&uri, &Document::new("foo\n", None));

    let params = make_params(uri, 0, 0);
    assert_eq!(goto_definition(params, &state).unwrap(), None);
}

#[test]
fn test_cursor_on_operator_returns_none() {
    // Cursor on `<-`, not on an identifier use: nothing to resolve.
    let uri = test_path("file.R");
    let state = make_state(&uri, &Document::new("foo <- 1\n", None));

    // Cursor on the `<` of `<-` at column 4.
    let params = make_params(uri, 0, 4);
    assert_eq!(goto_definition(params, &state).unwrap(), None);
}

#[test]
fn test_unlinked_cross_file_returns_none() {
    // `foo` is defined in another open file, but this file doesn't `source()`
    // it, so R semantics can't reach it. goto-def is precise: it returns `None`
    // rather than guessing by name across the workspace like the legacy ark handler.
    let uses_uri = test_path("uses.R");
    let defs_uri = test_path("defs.R");
    let state = make_state_with(&[(&uses_uri, "foo\n"), (&defs_uri, "foo <- function() 1\n")]);

    let params = make_params(uses_uri, 0, 0);
    assert_eq!(goto_definition(params, &state).unwrap(), None);
}

#[test]
fn test_resolves_across_source_directive() {
    // `script.R` sources `helpers.R`; goto-def on the forwarded `helper` use
    // lands in `helpers.R`. Exercises the cross-file branch of
    // `definition_to_link` (the target file's own line index + URL). The
    // resolution itself is covered exhaustively by `oak_db`'s `file_resolve_at`
    // tests; this checks the goto-def wiring on top of it.
    let script_uri = test_path("script.R");
    let helpers_uri = test_path("helpers.R");
    let state = make_state_with(&[
        (&script_uri, "source(\"helpers.R\")\nhelper\n"),
        (&helpers_uri, "helper <- function() 1\n"),
    ]);

    let params = make_params(script_uri, 1, 0);
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
    let script_uri = test_path("script.R");
    let helpers_uri = test_path("helpers.R");
    let state = make_state_with(&[
        (&script_uri, "source(\"helpers.R\")\nfoo <- 1\nfoo\n"),
        (&helpers_uri, "foo <- function() 2\n"),
    ]);

    let params = make_params(script_uri.clone(), 2, 0);
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
    // When the sourced file binds the same name twice, goto-def offers both
    // candidate definitions, in definition order. The last link is the binding
    // R picks at runtime. Ranges are in the target file's coordinates.
    let script_uri = test_path("script.R");
    let helpers_uri = test_path("helpers.R");
    let state = make_state_with(&[
        (&script_uri, "source(\"helpers.R\")\nfn\n"),
        (
            &helpers_uri,
            "fn <- function() 'first'\nfn <- function() 'second'\n",
        ),
    ]);

    let params = make_params(script_uri, 1, 0);
    assert_matches!(
        goto_definition(params, &state).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 2);
            assert_eq!(links[0].target_uri, helpers_uri);
            assert_eq!(links[0].target_range, range((0, 0), (0, 2)));
            assert_eq!(links[1].target_uri, helpers_uri);
            assert_eq!(links[1].target_range, range((1, 0), (1, 2)));
        }
    );
}
