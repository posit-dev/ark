//! Unit tests for the cursor-classification and candidate-enumeration
//! queries that back find-references / rename: `Identifier::classify`,
//! `uses_of`, `member_uses`. Resolution itself is covered by
//! `file_resolve(_at)`; here we pin down classification and the raw range
//! collection.

use biome_rowan::TextRange;
use biome_rowan::TextSize;

use crate::tests::test_db::file_url;
use crate::tests::test_db::TestDb;
use crate::File;
use crate::Identifier;
use crate::MemberKind;
use crate::Name;

fn make_file(db: &mut TestDb, contents: &str) -> File {
    File::new(db, file_url("a.R"), contents.to_string(), None)
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
fn test_classify_on_namespace_is_none() {
    // `pkg::sym` namespace access isn't a renamable variable or a member.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "dplyr::mutate\n");

    assert!(Identifier::classify(&db, file, offset(7)).is_none());
}

// --- classify: members ---

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
            assert_eq!(name, "bar");
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
            assert_eq!(name, "bar");
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

// --- member_uses ---

#[test]
fn test_member_uses_collects_matching_kind() {
    // "foo$bar\nbaz$bar\n": both `bar` are `$` members.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\nbaz$bar\n");

    let ranges = file.member_uses(&db, "bar", MemberKind::Dollar);
    assert_eq!(ranges, vec![text_range(4, 7), text_range(12, 15)]);
}

#[test]
fn test_member_uses_filters_by_kind() {
    // "foo$bar\nfoo@bar\n": one `$bar`, one `@bar`.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\nfoo@bar\n");

    assert_eq!(file.member_uses(&db, "bar", MemberKind::Dollar), vec![
        text_range(4, 7)
    ]);
    assert_eq!(file.member_uses(&db, "bar", MemberKind::At), vec![
        text_range(12, 15)
    ]);
}

#[test]
fn test_member_uses_ignores_plain_identifier() {
    // A standalone `bar` is not a member; only the `$bar` matches.
    let mut db = TestDb::new();
    let file = make_file(&mut db, "bar\nfoo$bar\n");

    assert_eq!(file.member_uses(&db, "bar", MemberKind::Dollar), vec![
        text_range(8, 11)
    ]);
}

#[test]
fn test_member_uses_filters_by_name() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "foo$bar\nfoo$baz\n");

    assert_eq!(file.member_uses(&db, "bar", MemberKind::Dollar), vec![
        text_range(4, 7)
    ]);
}
