use std::collections::HashMap;

use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::TestDb;
use crate::ExportEntry;
use crate::File;
use crate::Script;
use crate::SourceNode;

fn make_file(db: &mut TestDb, name: &str, contents: &str) -> File {
    intern_file(db, file_url(name), contents.to_string(), None)
}

fn make_script(db: &mut TestDb, name: &str, contents: &str) -> Script {
    let file = make_file(db, name, contents);
    let script = Script::new(db, file);
    file.set_parent(db).to(Some(SourceNode::Script(script)));
    script
}

fn entries(db: &TestDb, file: File) -> HashMap<String, ExportEntry> {
    file.exports(db)
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn local_top_level_definitions_become_local_entries() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "a.R", "x <- 1\nf <- function() 2\n");

    let map = entries(&db, file);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("x"), Some(&ExportEntry::Local));
    assert_eq!(map.get("f"), Some(&ExportEntry::Local));
}

#[test]
fn nested_definitions_are_not_exported() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "a.R", "f <- function() { y <- 1; y }\n");

    let map = entries(&db, file);
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("f"));
    assert!(!map.contains_key("y"));
}

#[test]
fn source_call_to_unresolved_path_drops_forwarding() {
    let mut db = TestDb::new();
    // No Script registered for "helpers.R", so the source() call
    // produces no forwarding entry.
    let file = make_file(&mut db, "a.R", "source(\"helpers.R\")\nx <- 1\n");

    let map = entries(&db, file);
    assert_eq!(map.len(), 1);
    assert_eq!(map.get("x"), Some(&ExportEntry::Local));
}

#[test]
fn source_call_to_resolved_script_produces_import_entries() {
    let mut db = TestDb::new();

    let helpers = make_script(&mut db, "/w/helpers.R", "helper <- function() 1\n");
    let analysis = make_file(&mut db, "/w/analysis.R", "source(\"helpers.R\")\nx <- 1\n");

    let map = entries(&db, analysis);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("x"), Some(&ExportEntry::Local));
    assert_eq!(
        map.get("helper"),
        Some(&ExportEntry::Import {
            script: helpers,
            name: "helper".to_string(),
        })
    );
}

#[test]
fn local_definition_shadows_sourced_name() {
    let mut db = TestDb::new();
    let _helpers = make_script(&mut db, "/w/helpers.R", "shared <- 1\n");
    let analysis = make_file(
        &mut db,
        "/w/analysis.R",
        "source(\"helpers.R\")\nshared <- 2\n",
    );

    let map = entries(&db, analysis);
    assert_eq!(map.get("shared"), Some(&ExportEntry::Local));
}

#[test]
fn later_source_overrides_earlier_when_both_export_the_same_name() {
    let mut db = TestDb::new();

    // Both `b` and `c` define `dup`. R evaluates each `source()` in
    // sequence and the later one's assignment wins.
    let _b = make_script(&mut db, "/w/b.R", "dup <- 1\n");
    let c = make_script(&mut db, "/w/c.R", "dup <- 2\n");
    let a = make_file(&mut db, "/w/a.R", "source(\"b.R\")\nsource(\"c.R\")\n");

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&ExportEntry::Import {
            script: c,
            name: "dup".to_string(),
        })
    );
}

#[test]
fn local_after_sources_wins() {
    let mut db = TestDb::new();

    // sources first, then local. Local is the last assignment so it
    // ends up bound at end-of-file.
    let _b = make_script(&mut db, "/w/b.R", "dup <- 1\n");
    let _c = make_script(&mut db, "/w/c.R", "dup <- 2\n");
    let a = make_file(
        &mut db,
        "/w/a.R",
        "source(\"b.R\")\nsource(\"c.R\")\ndup <- 3\n",
    );

    let map = entries(&db, a);
    assert_eq!(map.get("dup"), Some(&ExportEntry::Local));
}

#[test]
fn later_source_after_local_overrides_it() {
    let mut db = TestDb::new();

    // Local in the middle is shadowed by the *later* source. Matches
    // R's runtime: each statement assigns in order; the last write
    // wins.
    let _b = make_script(&mut db, "/w/b.R", "dup <- 1\n");
    let c = make_script(&mut db, "/w/c.R", "dup <- 2\n");
    let a = make_file(
        &mut db,
        "/w/a.R",
        "source(\"b.R\")\ndup <- 3\nsource(\"c.R\")\n",
    );

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&ExportEntry::Import {
            script: c,
            name: "dup".to_string(),
        })
    );
}

#[test]
fn local_before_sources_is_overridden() {
    let mut db = TestDb::new();

    // Local first, sources later. Sources reassign, last source wins.
    let _b = make_script(&mut db, "/w/b.R", "dup <- 1\n");
    let c = make_script(&mut db, "/w/c.R", "dup <- 2\n");
    let a = make_file(
        &mut db,
        "/w/a.R",
        "dup <- 3\nsource(\"b.R\")\nsource(\"c.R\")\n",
    );

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&ExportEntry::Import {
            script: c,
            name: "dup".to_string(),
        })
    );
}

#[test]
fn source_call_inside_function_body_does_not_affect_top_level_exports() {
    let mut db = TestDb::new();

    // A function body's `source()` runs only when the function is
    // called; statically it injects into the function's runtime scope,
    // not into the file's top-level bindings.
    let _helpers = make_script(&mut db, "/w/helpers.R", "injected <- 1\n");
    let a = make_file(&mut db, "/w/a.R", "f <- function() source(\"helpers.R\")\n");

    let map = entries(&db, a);
    assert!(map.contains_key("f"));
    assert!(!map.contains_key("injected"));
}

#[test]
fn source_chain_forwards_through_two_files() {
    let mut db = TestDb::new();

    let _leaf = make_script(&mut db, "/w/leaf.R", "deep <- 1\n");
    let mid = make_script(&mut db, "/w/mid.R", "source(\"leaf.R\")\n");
    let top = make_file(&mut db, "/w/top.R", "source(\"mid.R\")\n");

    // top forwards `deep` through mid -> leaf. The forwarding entry on
    // top points to `mid` (the direct source target). Chasing the
    // chain via `File::resolve` lands on `leaf` (Local).
    let map = entries(&db, top);
    assert_eq!(
        map.get("deep"),
        Some(&ExportEntry::Import {
            script: mid,
            name: "deep".to_string(),
        })
    );
}

#[test]
fn cyclic_source_returns_empty_exports_without_panicking() {
    let mut db = TestDb::new();

    // a.R sources b.R; b.R sources a.R. Illegal user code. Both
    // `File::exports` and `File::semantic_index` have FallbackImmediate
    // cycle handlers and every cycle participant gets the fallback,
    // so each side's exports map ends up empty. The test pins the
    // observable behaviour at the `exports` surface and that the
    // call returns without panicking.
    let a_file = make_file(&mut db, "/w/a.R", "source(\"b.R\")\na_local <- 1\n");
    let b_file = make_file(&mut db, "/w/b.R", "source(\"a.R\")\nb_local <- 2\n");
    let a = Script::new(&db, a_file);
    let b = Script::new(&db, b_file);
    a_file.set_parent(&mut db).to(Some(SourceNode::Script(a)));
    b_file.set_parent(&mut db).to(Some(SourceNode::Script(b)));

    let a_map = entries(&db, a_file);
    let b_map = entries(&db, b_file);
    assert!(a_map.is_empty());
    assert!(b_map.is_empty());
}

#[test]
fn editing_function_body_keeps_exports_stable() {
    let mut db = TestDb::new();
    let file = make_file(&mut db, "a.R", "f <- function() 1\n");

    let initial = entries(&db, file);
    assert_eq!(db.executions("exports"), 1);

    // An edit inside the function body changes `semantic_index` but
    // not the file's exports (still just `f`).
    file.set_contents(&mut db)
        .to("f <- function() 2\n".to_string());
    let after = entries(&db, file);

    assert_eq!(initial, after);
    // `exports` re-executes (its input `semantic_index` changed) but
    // downstream consumers see the same value via salsa backdating.
}
