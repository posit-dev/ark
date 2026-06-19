use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::TestDb;
use crate::File;

/// File entities are created directly with `File::new` so these tests
/// stay focused on per-query behavior (caching, backdating) without
/// touching the orphan/workspace bucketing logic that's exercised in
/// `oak_storage/tests/`.
fn new_file(db: &mut TestDb, name: &str, contents: &str) -> File {
    File::new(db, file_path(name), contents.to_string(), None)
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

    file.set_contents(&mut db).to("x <- 2\n".to_string());
    let _ = file.semantic_index(&db);

    assert_eq!(db.executions("parse"), 2);
    assert_eq!(db.executions("semantic_index"), 2);
}

#[test]
fn test_semantic_index_matches_oak_semantic() {
    let db = TestDb::new();
    let source = "x <- 1\nx\n";
    let url = file_path("a.R");
    let file = File::new(&db, url.clone(), source.to_string(), None);

    let via_salsa = file.semantic_index(&db);

    let parse = aether_parser::parse(source, aether_parser::RParserOptions::default());
    let direct = oak_semantic::build_index(&parse.tree(), oak_semantic::NoopImportsResolver);

    assert_eq!(via_salsa, &direct);
}

#[test]
fn test_semantic_index_backdates_on_equivalent_reparse() {
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", "x <- 1\n");

    let _ = file.semantic_index(&db);
    assert_eq!(db.executions("parse"), 1);
    assert_eq!(db.executions("semantic_index"), 1);

    // Setting identical contents bumps the revision, so Salsa must
    // re-validate `parse`. But the result is structurally equal to the
    // previous one, so `SendNode`'s `PartialEq` lets Salsa backdate and
    // skip re-executing the downstream `semantic_index`.
    file.set_contents(&mut db).to("x <- 1\n".to_string());
    let _ = file.semantic_index(&db);

    assert_eq!(db.executions("parse"), 2);
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
    // only on `file_a.contents`, so they must not re-execute.
    file_b.set_contents(&mut db).to("y <- 99\n".to_string());
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
