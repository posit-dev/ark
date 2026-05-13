use crate::tests::test_db::file_url;
use crate::tests::test_db::TestDb;
use crate::File;

#[test]
fn parse_is_cached_across_calls() {
    let db = TestDb::new();
    let file = File::new(&db, file_url("a.R"), "x <- 1\n".to_string());

    let _ = file.parse(&db);
    assert_eq!(db.executions("parse"), 1);

    let _ = file.parse(&db);
    assert_eq!(db.executions("parse"), 1);
}

#[test]
fn semantic_index_is_cached_across_calls() {
    let db = TestDb::new();
    let file = File::new(&db, file_url("a.R"), "x <- 1\n".to_string());

    let _ = file.semantic_index(&db);
    let _ = file.semantic_index(&db);
    assert_eq!(db.executions("parse"), 1);
    assert_eq!(db.executions("semantic_index"), 1);
}

#[test]
fn changing_contents_reparses() {
    use salsa::Setter;

    let mut db = TestDb::new();
    let file = File::new(&db, file_url("a.R"), "x <- 1\n".to_string());

    let _ = file.semantic_index(&db);
    assert_eq!(db.executions("parse"), 1);
    assert_eq!(db.executions("semantic_index"), 1);

    file.set_contents(&mut db).to("x <- 2\n".to_string());
    let _ = file.semantic_index(&db);

    assert_eq!(db.executions("parse"), 2);
    assert_eq!(db.executions("semantic_index"), 2);
}

#[test]
fn semantic_index_matches_oak_semantic() {
    let db = TestDb::new();
    let source = "x <- 1\nx\n";
    let url = file_url("a.R");
    let file = File::new(&db, url.clone(), source.to_string());

    let via_salsa = file.semantic_index(&db);

    let parse = aether_parser::parse(source, aether_parser::RParserOptions::default());
    let direct = oak_semantic::build_index(&parse.tree(), &url, &mut oak_semantic::NoopResolver);

    assert_eq!(via_salsa, &direct);
}

#[test]
fn semantic_index_backdates_on_equivalent_reparse() {
    use salsa::Setter;

    let mut db = TestDb::new();
    let file = File::new(&db, file_url("a.R"), "x <- 1\n".to_string());

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
fn editing_one_file_does_not_invalidate_another() {
    use salsa::Setter;

    let mut db = TestDb::new();
    let file_a = File::new(&db, file_url("a.R"), "x <- 1\n".to_string());
    let file_b = File::new(&db, file_url("b.R"), "y <- 2\n".to_string());

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
fn distinct_files_have_distinct_semantic_indexes() {
    let db = TestDb::new();
    let file_a = File::new(&db, file_url("a.R"), "x <- 1\n".to_string());
    let file_b = File::new(&db, file_url("b.R"), "x <- 1\n".to_string());

    // Same contents, different `File` inputs: separate cache entries.
    assert!(file_a != file_b);

    let idx_a = file_a.semantic_index(&db);
    let idx_b = file_b.semantic_index(&db);

    // Each index records its own file URL, so they are not structurally
    // equal even though the source text matches.
    assert_ne!(*idx_a, *idx_b);
    assert_eq!(db.executions("parse"), 2);
    assert_eq!(db.executions("semantic_index"), 2);
}
