//! Unit tests for the cursor-classification and candidate-enumeration
//! queries that back find-references / rename: `Identifier::classify`,
//! `uses_of`, `member_uses_of`. Resolution itself is covered by
//! `file_resolve(_at)`; here we pin down classification and the raw range
//! collection.

use biome_rowan::TextRange;
use biome_rowan::TextSize;

use crate::tests::test_db::file_path;
use crate::tests::test_db::TestDb;
use crate::File;
use crate::FileRevision;
use crate::Identifier;
use crate::MemberKind;
use crate::Name;
use crate::NamespaceVisibility;

fn make_file(db: &mut TestDb, contents: &str) -> File {
    File::new(
        db,
        file_path("a.R"),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

fn offset(n: u32) -> TextSize {
    TextSize::from(n)
}

fn text_range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end))
}

// --- classify: variables ---

#[test]
fn test_classify_on_use_is_variable() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "x <- 1\nx\n");

    match Identifier::classify(&db, file, offset(7)) {
        Some(Identifier::Variable { name, range }) => {
            assert_eq!(name.text(&db).as_str(), "x");
            assert_eq!(range, text_range(7, 8));
        },
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn test_classify_on_def_is_variable() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "x <- 1\nx\n");

    match Identifier::classify(&db, file, offset(0)) {
        Some(Identifier::Variable { name, range }) => {
            assert_eq!(name.text(&db).as_str(), "x");
            assert_eq!(range, text_range(0, 1));
        },
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn test_classify_snaps_trailing_edge() {
    // Cursor at offset 8 is one past the use `x` (7..8); snapping pulls it
    // back onto the token.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "x <- 1\nx\n");

    match Identifier::classify(&db, file, offset(8)) {
        Some(Identifier::Variable { range, .. }) => assert_eq!(range, text_range(7, 8)),
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn test_classify_on_operator_is_none() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "x <- 1\n");

    // Cursor on `<-`.
    assert!(Identifier::classify(&db, file, offset(2)).is_none());
}

#[test]
fn test_classify_on_namespace_symbol() {
    // "dplyr::mutate": `dplyr` 0..5, `::` 5..7, `mutate` 7..13.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr::mutate\n");

    match Identifier::classify(&db, file, offset(7)) {
        Some(Identifier::NamespaceAccess {
            package,
            name,
            visibility,
        }) => {
            assert_eq!(package.text(&db).as_str(), "dplyr");
            assert_eq!(name.text(&db).as_str(), "mutate");
            assert_eq!(visibility, NamespaceVisibility::Exported);
        },
        other => panic!("expected NamespaceAccess, got {other:?}"),
    }
}

#[test]
fn test_classify_on_namespace_package() {
    // Cursor on the `dplyr` package half classifies as the same symbol
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr::mutate\n");

    match Identifier::classify(&db, file, offset(0)) {
        Some(Identifier::NamespaceAccess {
            package,
            name,
            visibility,
        }) => {
            assert_eq!(package.text(&db).as_str(), "dplyr");
            assert_eq!(name.text(&db).as_str(), "mutate");
            assert_eq!(visibility, NamespaceVisibility::Exported);
        },
        other => panic!("expected NamespaceAccess, got {other:?}"),
    }
}

#[test]
fn test_classify_inside_namespace_operator() {
    // "dplyr:::mutate": `:::` 5..8. A cursor strictly inside the operator
    // (offset 6) snaps onto the symbol.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr:::mutate\n");

    match Identifier::classify(&db, file, offset(6)) {
        Some(Identifier::NamespaceAccess {
            package,
            name,
            visibility,
        }) => {
            assert_eq!(package.text(&db).as_str(), "dplyr");
            assert_eq!(name.text(&db).as_str(), "mutate");
            assert_eq!(visibility, NamespaceVisibility::Internal);
        },
        other => panic!("expected NamespaceAccess, got {other:?}"),
    }
}

// --- classify: members ---

#[test]
fn test_classify_on_backticked_member_strips_backticks() {
    // "foo$`bar`": `$` 3..4, `` `bar` `` 4..9. The classified name is unquoted,
    // but the range spans the backticks.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$`bar`\n");

    match Identifier::classify(&db, file, offset(5)) {
        Some(Identifier::Member {
            name, name_range, ..
        }) => {
            assert_eq!(name.text(&db).as_str(), "bar");
            assert_eq!(name_range, text_range(4, 9));
        },
        other => panic!("expected Member, got {other:?}"),
    }
}

#[test]
fn test_classify_on_dollar_member() {
    // "foo$bar": `$` at 3..4, `bar` at 4..7.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\n");

    match Identifier::classify(&db, file, offset(4)) {
        Some(Identifier::Member {
            name,
            kind,
            operator_range,
            name_range,
        }) => {
            assert_eq!(name.text(&db).as_str(), "bar");
            assert_eq!(kind, MemberKind::Dollar);
            assert_eq!(operator_range, text_range(3, 4));
            assert_eq!(name_range, text_range(4, 7));
        },
        other => panic!("expected Member, got {other:?}"),
    }
}

#[test]
fn test_classify_on_at_member() {
    // "foo@bar": `@` at 3..4, `bar` at 4..7.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo@bar\n");

    match Identifier::classify(&db, file, offset(4)) {
        Some(Identifier::Member { name, kind, .. }) => {
            assert_eq!(name.text(&db).as_str(), "bar");
            assert_eq!(kind, MemberKind::At);
        },
        other => panic!("expected Member, got {other:?}"),
    }
}

#[test]
fn test_classify_on_member_lhs_is_variable() {
    // The LHS of `$` is a real variable use, not a member name.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\n");

    match Identifier::classify(&db, file, offset(0)) {
        Some(Identifier::Variable { name, range }) => {
            assert_eq!(name.text(&db).as_str(), "foo");
            assert_eq!(range, text_range(0, 3));
        },
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn test_classify_nested_dollar_by_cursor_position() {
    // "foo$bar$baz" parses as `(foo$bar)$baz`. `foo` 0..3, `$` 3..4, `bar`
    // 4..7, `$` 7..8, `baz` 8..11. The cursor picks out the base variable
    // and each member by position.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar$baz\n");

    match Identifier::classify(&db, file, offset(0)) {
        Some(Identifier::Variable { name, range }) => {
            assert_eq!(name.text(&db).as_str(), "foo");
            assert_eq!(range, text_range(0, 3));
        },
        other => panic!("expected Variable for `foo`, got {other:?}"),
    }

    match Identifier::classify(&db, file, offset(4)) {
        Some(Identifier::Member {
            name,
            operator_range,
            name_range,
            ..
        }) => {
            assert_eq!(name.text(&db).as_str(), "bar");
            assert_eq!(operator_range, text_range(3, 4));
            assert_eq!(name_range, text_range(4, 7));
        },
        other => panic!("expected Member for `bar`, got {other:?}"),
    }

    match Identifier::classify(&db, file, offset(8)) {
        Some(Identifier::Member {
            name,
            operator_range,
            name_range,
            ..
        }) => {
            assert_eq!(name.text(&db).as_str(), "baz");
            assert_eq!(operator_range, text_range(7, 8));
            assert_eq!(name_range, text_range(8, 11));
        },
        other => panic!("expected Member for `baz`, got {other:?}"),
    }
}

// --- uses_of ---

#[test]
fn test_uses_of_collects_uses_not_defs() {
    // "x <- 1\nx + x\n": def at 0..1 (excluded), uses at 7..8 and 11..12.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "x <- 1\nx + x\n");

    let ranges = file.uses_of(&db, Name::new(&db, "x"));
    assert_eq!(ranges, vec![text_range(7, 8), text_range(11, 12)]);
}

#[test]
fn test_uses_of_spans_scopes() {
    // A free use inside a function body is still collected.
    let mut db = TestDb::new();
    let source = "x <- 1\nf <- function() x\n";
    let file = make_file(&mut db, source);

    let inner_use = source.rfind('x').unwrap() as u32;
    let ranges = file.uses_of(&db, Name::new(&db, "x"));
    assert_eq!(ranges, vec![text_range(inner_use, inner_use + 1)]);
}

#[test]
fn test_uses_of_distinguishes_names() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "x <- 1\ny <- 2\nx\ny\n");

    let ranges = file.uses_of(&db, Name::new(&db, "x"));
    assert_eq!(ranges, vec![text_range(14, 15)]);
}

#[test]
fn test_uses_of_unknown_name_is_empty() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "x <- 1\nx\n");

    assert!(file.uses_of(&db, Name::new(&db, "nope")).is_empty());
}

// --- member_uses_of ---

#[test]
fn test_member_uses_of_collects_matching_kind() {
    // "foo$bar\nbaz$bar\n": both `bar` are `$` members.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\nbaz$bar\n");

    let ranges = file.member_uses_of(&db, "bar", MemberKind::Dollar);
    assert_eq!(ranges, vec![text_range(4, 7), text_range(12, 15)]);
}

#[test]
fn test_member_uses_of_recurses_into_nested_dollar() {
    // "foo$bar$baz": `(foo$bar)$baz`. The scan walks both the inner and outer
    // extract, so `bar` (4..7) and `baz` (8..11) each match on their own.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar$baz\n");

    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::Dollar), vec![
        text_range(4, 7)
    ]);
    assert_eq!(file.member_uses_of(&db, "baz", MemberKind::Dollar), vec![
        text_range(8, 11)
    ]);
}

#[test]
fn test_member_uses_of_recurses_through_braces_and_parens() {
    // `descendants()` walks the whole subtree, so members nested in a function
    // body / braces and in parens are found, not just top-level ones.
    // "f <- function() {\n  a$bar\n}\n(b$bar)\n": `a$bar` 22..25, `b$bar` 31..34.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "f <- function() {\n  a$bar\n}\n(b$bar)\n");

    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::Dollar), vec![
        text_range(22, 25),
        text_range(31, 34),
    ]);
}

#[test]
fn test_member_uses_of_filters_by_kind() {
    // "foo$bar\nfoo@bar\n": one `$bar`, one `@bar`.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\nfoo@bar\n");

    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::Dollar), vec![
        text_range(4, 7)
    ]);
    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::At), vec![
        text_range(12, 15)
    ]);
}

#[test]
fn test_member_uses_of_ignores_plain_identifier() {
    // A standalone `bar` is not a member; only the `$bar` matches.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "bar\nfoo$bar\n");

    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::Dollar), vec![
        text_range(8, 11)
    ]);
}

#[test]
fn test_member_uses_of_filters_by_name() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\nfoo$baz\n");

    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::Dollar), vec![
        text_range(4, 7)
    ]);
}

#[test]
fn test_member_uses_of_ignores_dots_selectors() {
    // `...` and `..1` are the dots mechanism, not member names. `foo$...` and
    // `foo$..1` yield no member reference, only the real `$bar` matches.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$...\nfoo$..1\nfoo$bar\n");

    assert!(file
        .member_uses_of(&db, "...", MemberKind::Dollar)
        .is_empty());
    assert!(file
        .member_uses_of(&db, "..1", MemberKind::Dollar)
        .is_empty());
    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::Dollar), vec![
        text_range(20, 23)
    ]);
}

#[test]
fn test_member_uses_of_normalizes_backticks_and_quotes() {
    // `foo$bar`, `` baz$`bar` ``, and `qux$"bar"` all name member `bar`, so a
    // search for the unquoted "bar" finds all three. The same stripping that
    // `classify` applies to the cursor's name is applied to each candidate, so
    // the forms cross-match. The quoted forms' ranges span the backticks/quotes.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\nbaz$`bar`\nqux$\"bar\"\n");

    assert_eq!(file.member_uses_of(&db, "bar", MemberKind::Dollar), vec![
        text_range(4, 7),
        text_range(12, 17),
        text_range(22, 27),
    ]);
}

// --- namespace_uses_of ---

#[test]
fn test_namespace_uses_of_collects_matching() {
    // "dplyr::mutate\ndplyr::mutate\n": RHS `mutate` at 7..13 and 21..27.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr::mutate\ndplyr::mutate\n");

    let ranges = file.namespace_uses_of(&db, "dplyr", "mutate");
    assert_eq!(ranges, vec![text_range(7, 13), text_range(21, 27)]);
}

#[test]
fn test_namespace_uses_of_matches_triple_colon() {
    // `:::` names the same symbol as `::`.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr::mutate\ndplyr:::mutate\n");

    let ranges = file.namespace_uses_of(&db, "dplyr", "mutate");
    assert_eq!(ranges, vec![text_range(7, 13), text_range(22, 28)]);
}

#[test]
fn test_namespace_uses_of_filters_by_namespace() {
    // `tidyr::mutate` is a different symbol than `dplyr::mutate`.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr::mutate\ntidyr::mutate\n");

    assert_eq!(file.namespace_uses_of(&db, "dplyr", "mutate"), vec![
        text_range(7, 13)
    ]);
}

#[test]
fn test_namespace_uses_of_filters_by_name() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr::mutate\ndplyr::filter\n");

    assert_eq!(file.namespace_uses_of(&db, "dplyr", "mutate"), vec![
        text_range(7, 13)
    ]);
}

#[test]
fn test_namespace_uses_of_ignores_bare_call() {
    // A bare `mutate` is not a namespace access; only `dplyr::mutate` matches.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "mutate\ndplyr::mutate\n");

    assert_eq!(file.namespace_uses_of(&db, "dplyr", "mutate"), vec![
        text_range(14, 20)
    ]);
}
