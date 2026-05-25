use assert_matches::assert_matches;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;

use crate::lsp::document::Document;
use crate::lsp::goto_definition::goto_definition;
use crate::lsp::indexer;
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

#[test]
fn test_goto_definition() {
    let code = "foo <- 42\nprint(foo)\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");

    let params = make_params(uri, 1, 6);

    assert_matches!(
        goto_definition(&doc, params).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(
                links[0].target_range,
                lsp_types::Range {
                    start: lsp_types::Position::new(0, 0),
                    end: lsp_types::Position::new(0, 3),
                }
            );
        }
    );
}

#[test]
fn test_goto_definition_prefers_local_symbol() {
    let code = "foo <- 1\nfoo\n";
    let doc = Document::new(code, None);
    let uri = test_path("file.R");

    let params = make_params(uri.clone(), 1, 0);

    assert_matches!(
        goto_definition(&doc, params).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_uri, uri);
            assert_eq!(
                links[0].target_range,
                lsp_types::Range {
                    start: lsp_types::Position::new(0, 0),
                    end: lsp_types::Position::new(0, 3),
                }
            );
        }
    );
}

#[test]
fn test_fallback_empty_indexer_returns_self_target() {
    // doc2 uses `foo` but has no local definition, and the workspace
    // indexer has no entry either. We expect a self-target link (the
    // cursor's own range) so the editor can still surface
    // find-references for the symbol.
    let _guard = indexer::ResetIndexerGuard;

    let doc = Document::new("foo\n", None);
    let uri = test_path("file.R");

    let params = make_params(uri.clone(), 0, 0);
    assert_matches!(
        goto_definition(&doc, params).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, uri);
            let expected = lsp_types::Range {
                start: lsp_types::Position::new(0, 0),
                end: lsp_types::Position::new(0, 3),
            };
            assert_eq!(links[0].target_range, expected);
            assert_eq!(links[0].origin_selection_range, Some(expected));
        }
    );
}

#[test]
fn test_fallback_resolves_cross_file() {
    // A free variable that the intra-file resolver can't bind falls back
    // to the workspace indexer, which finds the definition in another
    // indexed file.
    let _guard = indexer::ResetIndexerGuard;

    let doc1 = Document::new("foo <- function() 1\n", None);
    let uri1 = test_path("defs.R");
    indexer::update(&doc1, &uri1).unwrap();

    let doc2 = Document::new("foo\n", None);
    let uri2 = test_path("uses.R");

    let params = make_params(uri2, 0, 0);
    assert_matches!(
        goto_definition(&doc2, params).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_uri, uri1);
        }
    );
}

#[test]
fn test_fallback_skipped_when_local_def_wins() {
    // When the symbol has a binding within the file, the within-file
    // result is returned and the indexer fallback isn't consulted.
    let _guard = indexer::ResetIndexerGuard;

    let other = Document::new("foo <- function() 'other'\n", None);
    let other_uri = test_path("other.R");
    indexer::update(&other, &other_uri).unwrap();

    let code = "foo <- 1\nfoo\n";
    let doc = Document::new(code, None);
    let uri = test_path("main.R");

    let params = make_params(uri.clone(), 1, 0);
    assert_matches!(
        goto_definition(&doc, params).unwrap(),
        Some(GotoDefinitionResponse::Link(ref links)) => {
            assert_eq!(links[0].target_uri, uri);
            assert_eq!(
                links[0].target_range,
                lsp_types::Range {
                    start: lsp_types::Position::new(0, 0),
                    end: lsp_types::Position::new(0, 3),
                }
            );
        }
    );
}
