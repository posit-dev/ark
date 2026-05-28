use tower_lsp::lsp_types;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::ReferenceContext;
use tower_lsp::lsp_types::ReferenceParams;

use super::utils::make_state;
use super::utils::range;
use crate::lsp::document::Document;
use crate::lsp::find_references::find_references;
use crate::lsp::util::test_path;

fn make_params(
    uri: lsp_types::Url,
    line: u32,
    character: u32,
    include_decl: bool,
) -> ReferenceParams {
    ReferenceParams {
        text_document_position: lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            position: lsp_types::Position::new(line, character),
        },
        context: ReferenceContext {
            include_declaration: include_decl,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

#[test]
fn test_intra_file_use_and_def() {
    let code = "foo <- 1\nfoo + foo\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    // Cursor on the first use at (1, 0)
    let params = make_params(uri.clone(), 1, 0, true);
    let locs = find_references(params, &state).unwrap();
    let expected: Vec<Location> = vec![
        Location::new(uri.clone(), range((0, 0), (0, 3))),
        Location::new(uri.clone(), range((1, 0), (1, 3))),
        Location::new(uri, range((1, 6), (1, 9))),
    ];
    assert_eq!(locs, expected);
}

#[test]
fn test_excludes_declaration() {
    let code = "foo <- 1\nfoo\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    let params = make_params(uri.clone(), 1, 0, false);
    let locs = find_references(params, &state).unwrap();
    assert_eq!(locs, vec![Location::new(uri, range((1, 0), (1, 3)))]);
}

#[test]
fn test_no_identifier_returns_empty() {
    let code = "x <- 1\n";
    let doc = Document::new(code, None);
    let uri = test_path("test.R");
    let state = make_state(&uri, &doc);

    // Cursor on the `<-` operator: no identifier, no refs.
    let params = make_params(uri, 0, 3, true);
    let locs = find_references(params, &state).unwrap();
    assert!(locs.is_empty());
}

#[test]
fn test_cursor_past_trailing_edge_resolves_intra_file() {
    // Locally-bound symbol with cursor at the trailing edge of the use
    // (typical for double-click then "find references"). The intra-file
    // pass must catch this via `Identifier::classify`'s offset retry --
    // otherwise it would fall through to the textual walk and pollute
    // the result with cross-file noise.
    let code = "foo <- 1\nfoo\n";
    let doc = Document::new(code, None);
    let uri = test_path("intra_trailing.R");
    let state = make_state(&uri, &doc);

    // Cursor at line 1, column 3: one past the last character of `foo`
    // (which spans columns 0..3).
    let params = make_params(uri.clone(), 1, 3, true);
    let locs = find_references(params, &state).unwrap();

    let expected: Vec<Location> = vec![
        Location::new(uri.clone(), range((0, 0), (0, 3))),
        Location::new(uri, range((1, 0), (1, 3))),
    ];
    assert_eq!(locs, expected);
}

#[test]
fn test_cursor_past_trailing_edge_resolves_via_fallback() {
    // Some LSP clients send the cursor one past the last character of a
    // selected identifier (typical for double-click then "find references").
    // We exercise this through the cross-file fallback because `mutate` is
    // unbound: the intra-file pass returns nothing, and `build_context`'s
    // boundary retry pulls the cursor back one column onto the identifier
    // before the textual walk runs.
    let dir = tempfile::tempdir().unwrap();

    let file1_path = dir.path().join("a.R");
    std::fs::write(&file1_path, "mutate\n").unwrap();
    let file1_uri = lsp_types::Url::from_file_path(&file1_path).unwrap();

    let file2_path = dir.path().join("b.R");
    std::fs::write(&file2_path, "mutate\n").unwrap();
    let file2_uri = lsp_types::Url::from_file_path(&file2_path).unwrap();

    let doc1 = Document::new("mutate\n", None);
    let mut state = make_state(&file1_uri, &doc1);
    state.workspace.folders = vec![lsp_types::Url::from_directory_path(dir.path()).unwrap()];

    // Cursor at column 6: one past the last character of `mutate` (which
    // spans columns 0..6).
    let params = make_params(file1_uri.clone(), 0, 6, true);
    let locs = find_references(params, &state).unwrap();

    assert!(locs
        .iter()
        .any(|l| l.uri == file1_uri && l.range == range((0, 0), (0, 6))));
    assert!(locs
        .iter()
        .any(|l| l.uri == file2_uri && l.range == range((0, 0), (0, 6))));
}

#[test]
fn test_cross_file_walks_for_unbound_symbol() {
    // file1 uses `mutate` but doesn't define it locally. file2 also has
    // `mutate`. Cursor on file1's `mutate` -- intra-file returns nothing
    // (unbound), so the cross-file textual walk fires and picks up
    // file2's occurrences. This is not ideal behaviour, it's part of the legacy
    // Ark fallback handling.
    let dir = tempfile::tempdir().unwrap();

    let file1_path = dir.path().join("a.R");
    std::fs::write(&file1_path, "mutate\n").unwrap();
    let file1_uri = lsp_types::Url::from_file_path(&file1_path).unwrap();

    let file2_path = dir.path().join("b.R");
    std::fs::write(&file2_path, "mutate\nmutate + 1\n").unwrap();
    let file2_uri = lsp_types::Url::from_file_path(&file2_path).unwrap();

    let doc1 = Document::new("mutate\n", None);
    let mut state = make_state(&file1_uri, &doc1);
    state.workspace.folders = vec![lsp_types::Url::from_directory_path(dir.path()).unwrap()];

    let params = make_params(file1_uri.clone(), 0, 0, true);
    let locs = find_references(params, &state).unwrap();

    // The current file's occurrence (textual walk picks it up too,
    // since intra-file resolution gave nothing to dedup against).
    assert!(locs
        .iter()
        .any(|l| l.uri == file1_uri && l.range == range((0, 0), (0, 6))));
    // file2's two `mutate` occurrences.
    assert!(locs
        .iter()
        .any(|l| l.uri == file2_uri && l.range == range((0, 0), (0, 6))));
    assert!(locs
        .iter()
        .any(|l| l.uri == file2_uri && l.range == range((1, 0), (1, 6))));
}

#[test]
fn test_cross_file_walk_respects_dollar_kind() {
    // Cursor on `bar` in `foo$bar`. Intra-file gives nothing (member names
    // aren't tracked by the semantic index), so the cross-file walk fires.
    // The walk's `ReferenceKind` discrimination should match `bar` only
    // when it's also the RHS of a `$`, not plain identifier occurrences.
    let dir = tempfile::tempdir().unwrap();

    let file1_path = dir.path().join("a.R");
    std::fs::write(&file1_path, "foo <- list()\nfoo$bar\n").unwrap();
    let file1_uri = lsp_types::Url::from_file_path(&file1_path).unwrap();

    let file2_path = dir.path().join("b.R");
    std::fs::write(&file2_path, "foo$bar\nbar\n").unwrap();
    let file2_uri = lsp_types::Url::from_file_path(&file2_path).unwrap();

    let doc1 = Document::new("foo <- list()\nfoo$bar\n", None);
    let mut state = make_state(&file1_uri, &doc1);
    state.workspace.folders = vec![lsp_types::Url::from_directory_path(dir.path()).unwrap()];

    // Cursor on `bar` (RHS of `$`) in file1, line 1 col 4.
    let params = make_params(file1_uri.clone(), 1, 4, true);
    let locs = find_references(params, &state).unwrap();

    // file1's own `foo$bar` `bar`.
    assert!(locs
        .iter()
        .any(|l| l.uri == file1_uri && l.range == range((1, 4), (1, 7))));
    // file2's `foo$bar` `bar` (Dollar kind, matches).
    assert!(locs
        .iter()
        .any(|l| l.uri == file2_uri && l.range == range((0, 4), (0, 7))));
    // file2's plain `bar` (Symbol kind, does NOT match).
    assert!(!locs
        .iter()
        .any(|l| l.uri == file2_uri && l.range == range((1, 0), (1, 3))));
}

#[test]
fn test_fixme_cross_file_walk_on_namespace_access() {
    // Cursor on `mutate` in `dplyr::mutate`. Intra-file gives nothing
    // (NamespaceAccess), so the cross-file walk fires. The kind is
    // `Symbol` (parent is a namespace expression, not an extract
    // operator), so the walk matches other plain-identifier occurrences
    // of `mutate` but not `foo$mutate`. This is the noisy fallback
    // behavior; cross-file resolution will refine it later.
    let dir = tempfile::tempdir().unwrap();

    let file1_path = dir.path().join("a.R");
    std::fs::write(&file1_path, "dplyr::mutate\n").unwrap();
    let file1_uri = lsp_types::Url::from_file_path(&file1_path).unwrap();

    let file2_path = dir.path().join("b.R");
    std::fs::write(&file2_path, "mutate\nfoo$mutate\n").unwrap();
    let file2_uri = lsp_types::Url::from_file_path(&file2_path).unwrap();

    let doc1 = Document::new("dplyr::mutate\n", None);
    let mut state = make_state(&file1_uri, &doc1);
    state.workspace.folders = vec![lsp_types::Url::from_directory_path(dir.path()).unwrap()];

    // Cursor on `mutate` in `dplyr::mutate`, line 0 col 7.
    let params = make_params(file1_uri.clone(), 0, 7, true);
    let locs = find_references(params, &state).unwrap();

    // file1's own `mutate` in `dplyr::mutate`.
    assert!(locs
        .iter()
        .any(|l| l.uri == file1_uri && l.range == range((0, 7), (0, 13))));
    // file2's plain `mutate` (Symbol kind, matches).
    assert!(locs
        .iter()
        .any(|l| l.uri == file2_uri && l.range == range((0, 0), (0, 6))));
    // file2's `foo$mutate` `mutate` (Dollar kind, does NOT match).
    assert!(!locs
        .iter()
        .any(|l| l.uri == file2_uri && l.range == range((1, 4), (1, 10))));
}

#[test]
fn test_intra_file_match_merges_with_cross_file_walk() {
    // file1 defines + uses `foo` locally. file2 has unrelated `foo`s.
    // Cursor on file1's def: intra-file resolves precisely for file1
    // (def + use), and the cross-file textual walk picks up file2's
    // same-name occurrences. Until cross-file resolution lands those
    // file2 hits are unrefined and they may belong to a different
    // binding. TODO(salsa)
    let dir = tempfile::tempdir().unwrap();

    let file1_path = dir.path().join("a.R");
    std::fs::write(&file1_path, "foo <- 1\nfoo\n").unwrap();
    let file1_uri = lsp_types::Url::from_file_path(&file1_path).unwrap();

    let file2_path = dir.path().join("b.R");
    std::fs::write(&file2_path, "foo <- 99\nfoo\n").unwrap();
    let file2_uri = lsp_types::Url::from_file_path(&file2_path).unwrap();

    let doc1 = Document::new("foo <- 1\nfoo\n", None);
    let mut state = make_state(&file1_uri, &doc1);
    state.workspace.folders = vec![lsp_types::Url::from_directory_path(dir.path()).unwrap()];

    let params = make_params(file1_uri.clone(), 0, 0, true);
    let locs = find_references(params, &state).unwrap();

    // file1's two precise refs from intra-file, plus file2's two textual
    // matches. Sort for a deterministic comparison: WalkDir traversal
    // order isn't guaranteed across platforms.
    let mut actual = locs;
    actual.sort_by_key(|l| (l.uri.to_string(), l.range.start));
    let expected = vec![
        Location::new(file1_uri.clone(), range((0, 0), (0, 3))),
        Location::new(file1_uri, range((1, 0), (1, 3))),
        Location::new(file2_uri.clone(), range((0, 0), (0, 3))),
        Location::new(file2_uri, range((1, 0), (1, 3))),
    ];
    assert_eq!(actual, expected);
}
