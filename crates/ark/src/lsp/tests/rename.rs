use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::PrepareRenameResponse;
use tower_lsp::lsp_types::RenameParams;
use tower_lsp::lsp_types::TextDocumentPositionParams;
use tower_lsp::lsp_types::TextEdit;

use super::utils::insert_file;
use super::utils::make_state;
use super::utils::range;
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
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

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
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_prepare_params(uri, 0, 7);
    assert!(prepare_rename(params, &state).unwrap().is_none());
}

#[test]
fn test_edits_keyed_by_verbatim_url() {
    // The `WorkspaceEdit` must key its changes on the buffer's verbatim URL,
    // not a normalised round-trip, or the editor won't match them to the file.
    let code = "foo <- 1\nfoo\n";
    let uri = lsp_types::Url::parse("file:///C:/proj//file.R").unwrap();
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 0, 0, "bar");
    let edit = rename(params, &state).unwrap().unwrap();

    let changes = edit.changes.expect("changes map");
    assert!(changes.contains_key(&uri));
    assert_ne!(FilePath::from_url(&uri).to_url(), uri);
}

#[test]
fn test_rename_emits_edits_for_def_and_uses() {
    let code = "foo <- 1\nfoo + foo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 0, 0, "bar");
    let edit = rename(params, &state).unwrap().unwrap();

    let changes = edit.changes.expect("changes map");
    assert_eq!(changes.len(), 1);
    let mut edits = changes.get(&uri).expect("edits for uri").clone();
    edits.sort_by_key(|e| e.range.start);
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
    assert_eq!(edits, expected);
}

#[test]
fn test_rename_excludes_independent_binding_in_other_file() {
    // file2 has its own separate `foo` -- rename of file1's `foo` should
    // NOT touch file2's independent binding.
    let code1 = "foo <- 1\nfoo\n";
    let uri1 = test_path("a.R");
    let mut state = make_state(&uri1, code1);

    // file2 has its own separate `foo` -- rename of file1's `foo` should
    // NOT touch file2's independent binding.
    let code2 = "foo <- 99\nfoo\n";
    let uri2 = test_path("b.R");
    insert_file(&mut state, &uri2, code2);

    let params = make_rename_params(uri1.clone(), 0, 0, "bar");
    let edit = rename(params, &state).unwrap().unwrap();

    let changes = edit.changes.expect("changes map");
    // Only file1 should be in the changes (file2 has a different binding).
    assert_eq!(changes.len(), 1);
    assert!(changes.contains_key(&uri1));
    assert!(!changes.contains_key(&uri2));
}

#[test]
fn test_rename_cross_file_via_source() {
    // helpers.R defines `helper`; script.R sources it and uses it.
    // After registering both in a workspace root, rename spans both files.
    let code1 = "helper <- function() 1\n";
    let uri1 = test_path("helpers.R");
    let mut state = make_state(&uri1, code1);

    let code2 = "source(\"helpers.R\")\nhelper\n";
    let uri2 = test_path("script.R");
    insert_file(&mut state, &uri2, code2);

    // Register both files in a workspace root whose path is the temp
    // directory. `anchor_dir` uses the root path as the anchor, so
    // `source("helpers.R")` resolves to `<tmpdir>/helpers.R`.
    let fp1 = FilePath::from_url(&uri1);
    let fp2 = FilePath::from_url(&uri2);
    let file1 = state.db.file_by_path(&fp1).unwrap();
    let file2 = state.db.file_by_path(&fp2).unwrap();
    let root_path = FilePath::from_path_buf(std::env::temp_dir()).unwrap();
    let root = Root::new(
        &state.db,
        root_path,
        RootKind::Workspace,
        vec![file1, file2],
        vec![],
    );
    state
        .db
        .workspace_roots()
        .set_roots(&mut state.db)
        .to(vec![root]);

    // Cursor on `helper` use in script.R (line 1, col 0).
    let params = make_rename_params(uri2.clone(), 1, 0, "renamed");
    let edit = rename(params, &state).unwrap().unwrap();

    let changes = edit.changes.expect("changes map");
    assert_eq!(changes.len(), 2);

    let mut edits1 = changes[&uri1].clone();
    edits1.sort_by_key(|e| e.range.start);
    assert_eq!(edits1, vec![TextEdit {
        range: range((0, 0), (0, 6)),
        new_text: "renamed".to_string(),
    }]);

    let mut edits2 = changes[&uri2].clone();
    edits2.sort_by_key(|e| e.range.start);
    assert_eq!(edits2, vec![TextEdit {
        range: range((1, 0), (1, 6)),
        new_text: "renamed".to_string(),
    }]);
}

#[test]
fn test_rename_string_assignment_keeps_quotes() {
    // `"foo" <- 1` binds `foo` via a string. Renaming keeps it a string rather
    // than unquoting to `bar <- 1`, so the binding form is preserved.
    let code = "\"foo\" <- 1\nfoo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 1, 0, "bar");
    let edit = rename(params, &state).unwrap().unwrap();

    let mut edits = edit.changes.unwrap().remove(&uri).unwrap();
    edits.sort_by_key(|e| e.range.start);
    let expected: Vec<TextEdit> = vec![
        TextEdit {
            range: range((0, 0), (0, 5)),
            new_text: "\"bar\"".to_string(),
        },
        TextEdit {
            range: range((1, 0), (1, 3)),
            new_text: "bar".to_string(),
        },
    ];
    assert_eq!(edits, expected);
}

#[test]
fn test_rename_assign_call_keeps_quotes() {
    // `assign("foo", 1)` binds `foo` via a string argument. Unquoting it to
    // `assign(bar, 1)` would change the program, so the rename stays quoted.
    let code = "assign(\"foo\", 1)\nfoo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 1, 0, "bar");
    let edit = rename(params, &state).unwrap().unwrap();

    let mut edits = edit.changes.unwrap().remove(&uri).unwrap();
    edits.sort_by_key(|e| e.range.start);
    let expected: Vec<TextEdit> = vec![
        TextEdit {
            range: range((0, 7), (0, 12)),
            new_text: "\"bar\"".to_string(),
        },
        TextEdit {
            range: range((1, 0), (1, 3)),
            new_text: "bar".to_string(),
        },
    ];
    assert_eq!(edits, expected);
}

#[test]
fn test_rename_assign_call_preserves_single_quote_delimiter() {
    let code = "assign('foo', 1)\nfoo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 1, 0, "bar");
    let edit = rename(params, &state).unwrap().unwrap();

    let def_edit = edit
        .changes
        .unwrap()
        .remove(&uri)
        .unwrap()
        .into_iter()
        .find(|e| e.range.start.line == 0)
        .unwrap();
    assert_eq!(def_edit.new_text, "'bar'");
}

#[test]
fn test_rename_string_bound_symbol_to_spaced_name_stays_a_string() {
    // A name needing backticks as an identifier is a plain string in the
    // string-form site: `"foo" <- 1` -> `"new name" <- 1`, use -> `` `new name` ``.
    let code = "\"foo\" <- 1\nfoo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 1, 0, "new name");
    let edit = rename(params, &state).unwrap().unwrap();

    let mut edits = edit.changes.unwrap().remove(&uri).unwrap();
    edits.sort_by_key(|e| e.range.start);
    assert_eq!(edits[0].new_text, "\"new name\"");
    assert_eq!(edits[1].new_text, "`new name`");
}

#[test]
fn test_rename_assign_call_to_non_syntactic_name_stays_a_string() {
    // A non-syntactic target lands verbatim inside the quotes,
    // `assign("non-syntactic", 1)`, with no backticks (a string holds any name).
    // The bare-identifier use of the same symbol still gets backticks.
    let code = "assign(\"foo\", 1)\nfoo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 1, 0, "non-syntactic");
    let edit = rename(params, &state).unwrap().unwrap();

    let mut edits = edit.changes.unwrap().remove(&uri).unwrap();
    edits.sort_by_key(|e| e.range.start);
    assert_eq!(edits[0].new_text, "\"non-syntactic\"");
    assert_eq!(edits[1].new_text, "`non-syntactic`");
}

#[test]
fn test_rename_to_reserved_word_errors() {
    let code = "foo <- 1\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri, 0, 0, "if");
    let err = rename(params, &state).unwrap_err();
    assert!(err.to_string().contains("reserved"));
}

#[test]
fn test_rename_to_name_with_space_wraps_in_backticks() {
    let code = "foo <- 1\nfoo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 0, 0, "new name");
    let edit = rename(params, &state).unwrap().unwrap();
    let edits = edit.changes.unwrap().remove(&uri).unwrap();
    assert!(edits.iter().all(|e| e.new_text == "`new name`"));
}

#[test]
fn test_rename_to_name_with_starting_digit_wraps_in_backticks() {
    let code = "foo <- 1\nfoo\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri.clone(), 0, 0, "1foo");
    let edit = rename(params, &state).unwrap().unwrap();
    let edits = edit.changes.unwrap().remove(&uri).unwrap();
    assert!(edits.iter().all(|e| e.new_text == "`1foo`"));
}

#[test]
fn test_rename_to_empty_name_errors() {
    let code = "foo <- 1\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri, 0, 0, "");
    let err = rename(params, &state).unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn test_rename_to_name_with_backtick_errors() {
    let code = "foo <- 1\n";
    let uri = test_path("test.R");
    let state = make_state(&uri, code);

    let params = make_rename_params(uri, 0, 0, "foo`bar");
    let err = rename(params, &state).unwrap_err();
    assert!(err.to_string().contains("backtick"));
}
