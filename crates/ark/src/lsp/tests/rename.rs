use tower_lsp::lsp_types;
use tower_lsp::lsp_types::PrepareRenameResponse;
use tower_lsp::lsp_types::RenameParams;
use tower_lsp::lsp_types::TextDocumentPositionParams;
use tower_lsp::lsp_types::TextEdit;

use super::utils::make_state;
use super::utils::range;
use crate::lsp::document::Document;
use crate::lsp::rename::prepare_rename;
use crate::lsp::rename::rename;
use crate::lsp::util::test_path;

fn make_prepare_params(
    uri: lsp_types::Url,
    line: u32,
    character: u32,
) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: lsp_types::TextDocumentIdentifier { uri },
        position: lsp_types::Position::new(line, character),
    }
}

fn make_rename_params(
    uri: lsp_types::Url,
    line: u32,
    character: u32,
    new_name: &str,
) -> RenameParams {
    RenameParams {
        text_document_position: TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            position: lsp_types::Position::new(line, character),
        },
        new_name: new_name.to_string(),
        work_done_progress_params: Default::default(),
    }
}

#[test]
fn test_prepare_rename_returns_range_and_placeholder() {
    let code = "foo <- 1\nfoo\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    let params = make_prepare_params(uri, 0, 0);
    let result = prepare_rename(params, &state).unwrap().unwrap();

    let PrepareRenameResponse::RangeWithPlaceholder {
        range: r,
        placeholder,
    } = result
    else {
        panic!("expected RangeWithPlaceholder");
    };
    assert_eq!(r, range((0, 0), (0, 3)));
    assert_eq!(placeholder, "foo");
}

#[test]
fn test_prepare_rename_on_namespace_access_returns_none() {
    let code = "dplyr::mutate\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    let params = make_prepare_params(uri, 0, 7);
    assert!(prepare_rename(params, &state).unwrap().is_none());
}

#[test]
fn test_rename_emits_edits_for_def_and_uses() {
    let code = "foo <- 1\nfoo + foo\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    let params = make_rename_params(uri.clone(), 0, 0, "bar");
    let edit = rename(params, &state).unwrap().unwrap();

    let changes = edit.changes.expect("changes map");
    assert_eq!(changes.len(), 1);
    let edits = changes.get(&uri).expect("edits for uri");
    let expected: Vec<TextEdit> = vec![
        TextEdit {
            range: range((0, 0), (0, 3)),
            new_text: "bar".to_string(),
        },
        TextEdit {
            range: range((1, 0), (1, 3)),
            new_text: "bar".to_string(),
        },
        TextEdit {
            range: range((1, 6), (1, 9)),
            new_text: "bar".to_string(),
        },
    ];
    assert_eq!(edits, &expected);
}

#[test]
fn test_rename_to_reserved_word_errors() {
    // Reserved word as new name. `rename` returns an `Err`, which
    // `handle_rename` propagates to the client so the editor can show
    // the message inline.
    let code = "foo <- 1\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    let params = make_rename_params(uri, 0, 0, "if");
    let err = rename(params, &state).unwrap_err();
    assert!(err.to_string().contains("reserved"));
}

#[test]
fn test_rename_to_name_with_space_wraps_in_backticks() {
    let code = "foo <- 1\nfoo\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    let params = make_rename_params(uri.clone(), 0, 0, "new name");
    let edit = rename(params, &state).unwrap().unwrap();
    let edits = edit.changes.unwrap().remove(&uri).unwrap();
    assert!(edits.iter().all(|e| e.new_text == "`new name`"));
}
