use std::fs;

use aether_path::FilePath;
use biome_rowan::TextSize;
use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::TestDb;
use crate::File;
use crate::FileRevision;

/// File entities are created directly with `File::new` so these tests
/// stay focused on per-query behavior (caching, backdating) without
/// touching the orphan/workspace bucketing logic that's exercised in
/// `oak_storage/tests/`.
fn new_file(db: &mut TestDb, name: &str, contents: &str) -> File {
    File::new(
        db,
        file_path(name),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

#[test]
fn test_source_text_rereads_disk_when_revision_bumps() {
    // Guards the `revision` read inside `source_text`. A file with no override
    // reads from disk and memoizes the result. The bump is the only thing that
    // invalidates that memo, so a disk write done after the first read is
    // visible only because we bump the revision afterwards. Drop the `revision`
    // read from `source_text` and the second assertion sees the stale "v1".
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.R");
    fs::write(&path, "v1\n").unwrap();
    let url = FilePath::from_path_buf(path.clone()).unwrap();

    let mut db = TestDb::new();
    let file = File::new(&db, url, FileRevision::zero(), None, None);
    assert_eq!(file.source_text(&db), "v1\n");

    fs::write(&path, "v2\n").unwrap();
    file.set_revision(&mut db).to(FileRevision::from(1u128));
    assert_eq!(file.source_text(&db), "v2\n");
}

#[test]
fn test_parse_is_cached_across_calls() {
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", "x <- 1\n");

    let _ = file.parse(&db);
    assert_eq!(db.executions("parse"), 1);

    let _ = file.parse(&db);
    assert_eq!(db.executions("parse"), 1);
}

#[test]
fn test_line_index_is_cached_across_calls() {
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", "x <- 1\n");

    let _ = file.line_index(&db);
    assert_eq!(db.executions("line_index"), 1);

    let _ = file.line_index(&db);
    assert_eq!(db.executions("line_index"), 1);
}

#[test]
fn test_line_index_recomputes_on_content_change() {
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", "x <- 1\n");

    // One offset per line start: byte 0, then just past the `\n` at byte 6.
    assert_eq!(file.line_index(&db).newlines, vec![
        TextSize::from(0u32),
        TextSize::from(7u32)
    ]);
    assert_eq!(db.executions("line_index"), 1);

    file.set_source_text_override(&mut db)
        .to(Some("x\ny\nz\n".to_string()));
    assert_eq!(file.line_index(&db).newlines, vec![
        TextSize::from(0u32),
        TextSize::from(2u32),
        TextSize::from(4u32),
        TextSize::from(6u32),
    ]);
    assert_eq!(db.executions("line_index"), 2);
}

#[test]
fn test_semantic_index_is_cached_across_calls() {
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", "x <- 1\n");

    let _ = file.semantic_index(&db);
    let _ = file.semantic_index(&db);
    assert_eq!(db.executions("parse"), 1);
    assert_eq!(db.executions("semantic_index"), 1);
}

#[test]
fn test_changing_contents_reparses() {
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", "x <- 1\n");

    let _ = file.semantic_index(&db);
    assert_eq!(db.executions("parse"), 1);
    assert_eq!(db.executions("semantic_index"), 1);

    file.set_source_text_override(&mut db)
        .to(Some("x <- 2\n".to_string()));
    let _ = file.semantic_index(&db);

    assert_eq!(db.executions("parse"), 2);
    assert_eq!(db.executions("semantic_index"), 2);
}

#[test]
fn test_semantic_index_matches_oak_semantic() {
    let db = TestDb::new();
    let source = "x <- 1\nx\n";
    let url = file_path("a.R");
    let file = File::new(
        &db,
        url.clone(),
        FileRevision::zero(),
        Some(source.to_string()),
        None,
    );

    let via_salsa = file.semantic_index(&db);

    let parse = aether_parser::parse(source, aether_parser::RParserOptions::default());
    let direct = oak_semantic::build_index(&parse.tree(), oak_semantic::NoopImportsResolver);

    assert_eq!(via_salsa, &direct);
}

#[test]
fn test_semantic_index_backdates_on_equivalent_content_set() {
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", "x <- 1\n");

    let _ = file.semantic_index(&db);
    assert_eq!(db.executions("parse"), 1);
    assert_eq!(db.executions("semantic_index"), 1);

    // Setting `source_text_override` to an equal string: salsa sees the input
    // didn't change and backdates it, so neither `source_text`, `parse`, nor
    // `semantic_index` re-execute. The lazy `source_text` layer adds one more
    // backdating step than the old eager-contents design, but the net result
    // is strictly fewer re-executions.
    file.set_source_text_override(&mut db)
        .to(Some("x <- 1\n".to_string()));
    let _ = file.semantic_index(&db);

    assert_eq!(db.executions("parse"), 1);
    assert_eq!(db.executions("semantic_index"), 1);
}

#[test]
fn test_editing_one_file_does_not_invalidate_another() {
    let mut db = TestDb::new();
    let file_a = new_file(&mut db, "a.R", "x <- 1\n");
    let file_b = new_file(&mut db, "b.R", "y <- 2\n");

    let _ = file_a.semantic_index(&db);
    let _ = file_b.semantic_index(&db);
    assert_eq!(db.executions("parse"), 2);
    assert_eq!(db.executions("semantic_index"), 2);

    // Mutate `file_b` to an unrelated value. `file_a`'s queries depend
    // only on `file_a.source_text`, so they must not re-execute.
    file_b
        .set_source_text_override(&mut db)
        .to(Some("y <- 99\n".to_string()));
    let _ = file_a.semantic_index(&db);

    assert_eq!(db.executions("parse"), 2);
    assert_eq!(db.executions("semantic_index"), 2);

    // Re-querying `file_b` does re-run its pipeline.
    let _ = file_b.semantic_index(&db);
    assert_eq!(db.executions("parse"), 3);
    assert_eq!(db.executions("semantic_index"), 3);
}

#[test]
fn test_distinct_files_each_get_their_own_cache_entry() {
    let mut db = TestDb::new();
    let file_a = new_file(&mut db, "a.R", "x <- 1\n");
    let file_b = new_file(&mut db, "b.R", "x <- 1\n");

    // Different `File` inputs are distinct salsa entities.
    assert!(file_a != file_b);

    let idx_a = file_a.semantic_index(&db);
    let idx_b = file_b.semantic_index(&db);

    // Same contents produce structurally equal indexes (the index doesn't
    // carry the file URL anymore). Salsa still keys cache entries on the
    // `File` entity, so both queries actually ran.
    assert_eq!(*idx_a, *idx_b);
    assert_eq!(db.executions("parse"), 2);
    assert_eq!(db.executions("semantic_index"), 2);
}
