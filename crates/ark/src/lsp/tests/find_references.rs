use aether_path::FilePath;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::Location;
use tower_lsp_server::ls_types::ReferenceContext;
use tower_lsp_server::ls_types::ReferenceParams;
use tower_lsp_server::ls_types::Uri;
use url::Url;

use super::utils::insert_file;
use super::utils::make_state;
use super::utils::range;
use crate::lsp::find_references::find_references;
use crate::lsp::util::test_path;

fn make_params(uri: &Uri, line: u32, character: u32, include_decl: bool) -> ReferenceParams {
    ReferenceParams {
        text_document_position: lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            position: lsp_types::Position::new(line, character),
        },
        context: ReferenceContext {
            include_declaration: include_decl,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

/// Build the `Location` we expect `find_references()` to return for `uri`.
fn location(uri: &Uri, range: lsp_types::Range) -> Location {
    Location::new(uri.clone(), range)
}

fn at(loc: &Location, uri: &Uri) -> bool {
    &loc.uri == uri
}

#[test]
fn test_intra_file_use_and_def() {
    let code = "foo <- 1\nfoo + foo\n";
    let (state, uri) = make_state(test_path("test.R").as_str(), code);

    let params = make_params(&uri, 1, 0, true);
    let locs = find_references(params, &state).unwrap();
    let expected: Vec<Location> = vec![
        location(&uri, range((0, 0), (0, 3))),
        location(&uri, range((1, 0), (1, 3))),
        location(&uri, range((1, 6), (1, 9))),
    ];
    assert_eq!(locs, expected);
}

#[test]
fn test_excludes_declaration() {
    let code = "foo <- 1\nfoo\n";
    let (state, uri) = make_state(test_path("test.R").as_str(), code);

    let params = make_params(&uri, 1, 0, false);
    let locs = find_references(params, &state).unwrap();
    assert_eq!(locs, vec![location(&uri, range((1, 0), (1, 3)))]);
}

#[test]
fn test_no_identifier_returns_empty() {
    let code = "x <- 1\n";
    let (state, uri) = make_state(test_path("test.R").as_str(), code);

    let params = make_params(&uri, 0, 3, true);
    let locs = find_references(params, &state).unwrap();
    assert!(locs.is_empty());
}

#[test]
fn test_cursor_past_trailing_edge_resolves() {
    // Cursor at column 3: one past the last character of `foo` (0..3).
    let code = "foo <- 1\nfoo\n";
    let (state, uri) = make_state(test_path("trailing.R").as_str(), code);

    let params = make_params(&uri, 1, 3, true);
    let locs = find_references(params, &state).unwrap();

    let expected: Vec<Location> = vec![
        location(&uri, range((0, 0), (0, 3))),
        location(&uri, range((1, 0), (1, 3))),
    ];
    assert_eq!(locs, expected);
}

#[test]
fn test_unbound_symbol_returns_empty() {
    // `mutate` has no definition in the db: precise resolution gives nothing.
    let code = "mutate\n";
    let (state, uri) = make_state(test_path("a.R").as_str(), code);

    let params = make_params(&uri, 0, 0, true);
    let locs = find_references(params, &state).unwrap();
    assert!(locs.is_empty());
}

#[test]
fn test_different_binding_not_included() {
    // file2 has its own `foo` binding. Cursor on file1's `foo` -- the
    // resolve_at confirm step excludes file2's `foo`.
    let code1 = "foo <- 1\nfoo\n";
    let (mut state, uri1) = make_state(test_path("a.R").as_str(), code1);

    let code2 = "foo <- 99\nfoo\n";
    insert_file(&mut state, test_path("b.R").as_str(), code2);

    let params = make_params(&uri1, 0, 0, true);
    let locs = find_references(params, &state).unwrap();
    assert!(locs.iter().all(|l| at(l, &uri1)));
    assert_eq!(locs.len(), 2);
}

#[test]
fn test_function_scope_target_stays_in_file() {
    // Parameter `x` is function-scoped, so only file1 is searched.
    let code1 = "f <- function(x) {\n  x + 1\n}\n";
    let (mut state, uri1) = make_state(test_path("a.R").as_str(), code1);

    let code2 = "x <- 99\nx\n";
    insert_file(&mut state, test_path("b.R").as_str(), code2);

    // Cursor on the parameter `x` at line 0, column 14.
    let params = make_params(&uri1, 0, 14, true);
    let locs = find_references(params, &state).unwrap();

    // Only file1's parameter and its use. No file2 hits.
    let expected: Vec<Location> = vec![
        location(&uri1, range((0, 14), (0, 15))),
        location(&uri1, range((1, 2), (1, 3))),
    ];
    assert_eq!(locs, expected);
}

#[test]
fn test_cross_file_dollar_kind() {
    // Cursor on `bar` in `foo$bar` -- structural member scan matches all `$bar`
    // occurrences regardless of LHS, and excludes plain `bar` (non-member).
    let code1 = "foo <- list()\nfoo$bar\n";
    let (mut state, uri1) = make_state(test_path("a.R").as_str(), code1);

    let code2 = "foo$bar\nbaz$bar\nbar\n";
    let uri2 = insert_file(&mut state, test_path("b.R").as_str(), code2);

    // Cursor on `bar` (RHS of `$`) at line 1, col 4.
    let params = make_params(&uri1, 1, 4, true);
    let locs = find_references(params, &state).unwrap();

    // a.R's `foo$bar` member
    assert!(locs
        .iter()
        .any(|l| at(l, &uri1) && l.range == range((1, 4), (1, 7))));
    // b.R's `foo$bar` member (matches)
    assert!(locs
        .iter()
        .any(|l| at(l, &uri2) && l.range == range((0, 4), (0, 7))));
    // b.R's `baz$bar` member (matches -- LHS is irrelevant for member scan)
    assert!(locs
        .iter()
        .any(|l| at(l, &uri2) && l.range == range((1, 4), (1, 7))));
    // b.R's plain `bar` (Symbol kind, does NOT match)
    assert!(!locs
        .iter()
        .any(|l| at(l, &uri2) && l.range == range((2, 0), (2, 3))));
}

#[test]
fn test_locations_use_verbatim_url() {
    // `FilePath` normalisation would collapse the doubled slash, decode and
    // re-encode the percent-escaped drive colon with different casing, and
    // leave the encoded brackets raw, so a `Url` round-trip changes all three
    // wire strings below. Locations must carry the exact bytes the editor
    // opened the buffer with.
    let code = "foo <- 1\nfoo\n";
    for wire in [
        "file:///C:/proj//foo.R",
        "file:///c%3A/proj/foo.R",
        "file:///C:/proj/f%5B1%5D.R",
    ] {
        let (state, uri) = make_state(wire, code);

        let params = make_params(&uri, 1, 0, true);
        let locs = find_references(params, &state).unwrap();

        assert!(!locs.is_empty());
        assert!(locs.iter().all(|loc| loc.uri.as_str() == wire));

        // Confirm the round-trip really would have differed, so the check
        // above bites.
        let url = Url::parse(wire).unwrap();
        assert_ne!(FilePath::from_url(&url).to_url(), url);
    }
}

#[test]
fn test_cross_file_namespace_access() {
    // Cursor on `mutate` in `dplyr::mutate` -- structural scan matches every
    // `dplyr::mutate` across files. The namespace matters, so `tidyr::mutate`
    // is excluded. A bare `mutate()` call is excluded too: installed packages
    // aren't part of the resolution graph, so neither `dplyr::mutate` nor a
    // bare `mutate` lands on a shared definition we could compare. We match
    // `pkg::name` structurally instead.
    //
    // TODO(namespace-refs): once `resolve` consumes the `Package` / `From`
    // import layers, a bare `mutate` will resolve to dplyr's `mutate`, and this
    // can resolve-and-compare like the variable path (folding the structural
    // scan into it). A bare `mutate()` would then be a match here.
    let code1 = "dplyr::mutate\n";
    let (mut state, uri1) = make_state(test_path("a.R").as_str(), code1);

    let code2 = "dplyr::mutate\ntidyr::mutate\nmutate()\n";
    let uri2 = insert_file(&mut state, test_path("b.R").as_str(), code2);

    // Cursor on `mutate` (RHS of `::`) at line 0, col 7.
    let params = make_params(&uri1, 0, 7, true);
    let locs = find_references(params, &state).unwrap();

    // a.R's `dplyr::mutate`
    assert!(locs
        .iter()
        .any(|l| at(l, &uri1) && l.range == range((0, 7), (0, 13))));
    // b.R's `dplyr::mutate` (matches)
    assert!(locs
        .iter()
        .any(|l| at(l, &uri2) && l.range == range((0, 7), (0, 13))));
    // b.R's `tidyr::mutate` (different namespace, does NOT match)
    assert!(!locs
        .iter()
        .any(|l| at(l, &uri2) && l.range == range((1, 7), (1, 13))));
    // b.R's bare `mutate()` (not a namespace access, does NOT match) TODO
    assert!(!locs
        .iter()
        .any(|l| at(l, &uri2) && l.range == range((2, 0), (2, 6))));
}
