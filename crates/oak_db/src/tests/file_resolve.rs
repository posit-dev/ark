use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::TestDb;
use crate::File;
use crate::Name;
use crate::Script;
use crate::SourceNode;

fn make_file(db: &mut TestDb, name: &str, contents: &str) -> File {
    intern_file(db, file_url(name), contents.to_string(), None)
}

fn make_script(db: &mut TestDb, name: &str, contents: &str) -> (File, Script) {
    let file = make_file(db, name, contents);
    let script = Script::new(db, file);
    file.set_parent(db).to(Some(SourceNode::Script(script)));
    (file, script)
}

fn name<'db>(db: &'db TestDb, text: &str) -> Name<'db> {
    Name::new(db, text)
}

#[test]
fn resolve_local_name_lands_on_owning_file() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "/w/a.R", "x <- 1\n");

    let resolution = file.resolve(&db, name(&db, "x")).expect("x should resolve");
    assert_eq!(resolution.file, file);
    assert_eq!(resolution.name, "x");
    // `semantic_index.file_exports()` records the binding's *name*
    // range, just the `x` identifier in `x <- 1`.
    assert_eq!(usize::from(resolution.range.start()), 0);
    assert_eq!(usize::from(resolution.range.end()), 1);
}

#[test]
fn unresolved_name_returns_none() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "/w/a.R", "x <- 1\n");
    assert!(file.resolve(&db, name(&db, "nope")).is_none());
}

#[test]
fn resolve_chases_source_forwarding_to_origin_file() {
    let mut db = TestDb::new();

    let (helpers_file, _helpers_script) =
        make_script(&mut db, "/w/helpers.R", "helper <- function() 1\n");
    let analysis = make_file(&mut db, "/w/analysis.R", "source(\"helpers.R\")\n");

    let resolution = analysis
        .resolve(&db, name(&db, "helper"))
        .expect("helper should resolve through source()");

    assert_eq!(resolution.file, helpers_file);
    assert_eq!(resolution.name, "helper");
}

#[test]
fn resolve_chases_two_step_source_chain() {
    let mut db = TestDb::new();

    let (leaf_file, _leaf) = make_script(&mut db, "/w/leaf.R", "deep <- 1\n");
    let (_mid_file, _mid) = make_script(&mut db, "/w/mid.R", "source(\"leaf.R\")\n");
    let top = make_file(&mut db, "/w/top.R", "source(\"mid.R\")\n");

    let resolution = top
        .resolve(&db, name(&db, "deep"))
        .expect("deep should chase through mid -> leaf");

    assert_eq!(resolution.file, leaf_file);
    assert_eq!(resolution.name, "deep");
}

#[test]
fn resolve_is_cached_across_repeat_calls() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "/w/a.R", "x <- 1\n");

    let _ = file.resolve(&db, name(&db, "x"));
    let _ = file.resolve(&db, name(&db, "x"));

    // Tracked: the second call hits the salsa cache.
    assert_eq!(db.executions("resolve"), 1);
}

#[test]
fn resolve_in_cyclic_source_returns_none_without_panicking() {
    let mut db = TestDb::new();

    let (a_file, _a) = make_script(&mut db, "/w/a.R", "source(\"b.R\")\na_local <- 1\n");
    let (b_file, _b) = make_script(&mut db, "/w/b.R", "source(\"a.R\")\nb_local <- 2\n");

    // a.R sources b.R; b.R sources a.R. Both sides' `exports` cycle
    // to empty via `cycle_result`, so `resolve` returns `None` for
    // names that would otherwise be found in those exports. The
    // point of the test is that resolution terminates cleanly rather
    // than panicking.
    assert!(a_file.resolve(&db, name(&db, "a_local")).is_none());
    assert!(b_file.resolve(&db, name(&db, "b_local")).is_none());
}
