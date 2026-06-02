use std::collections::HashMap;

use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::ExportEntry;
use crate::File;

/// Build a workspace root at `/w` populated with the given scripts.
/// Returns the file handles in the same order. Registers the root with
/// `WorkspaceRoots` so `file_by_path` finds the files.
fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "w");
    let files: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| File::new(db, file_path(name), contents.to_string(), None))
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    files
}

fn entries(db: &TestDb, file: File) -> HashMap<String, Vec<ExportEntry>> {
    file.exports(db)
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_vec()))
        .collect()
}

#[test]
fn test_local_top_level_definitions_become_local_entries() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\nf <- function() 2\n")]);

    let map = entries(&db, files[0]);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("x"), Some(&vec![ExportEntry::Local]));
    assert_eq!(map.get("f"), Some(&vec![ExportEntry::Local]));
}

#[test]
fn test_nested_definitions_are_not_exported() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "f <- function() { y <- 1; y }\n")]);

    let map = entries(&db, files[0]);
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("f"));
    assert!(!map.contains_key("y"));
}

#[test]
fn test_source_call_to_unresolved_path_drops_forwarding() {
    let mut db = TestDb::new();
    // Only `a.R` is registered; `helpers.R` has no entry, so the
    // `source()` call produces no forwarding `Import`.
    let files = setup_workspace(&mut db, &[("w/a.R", "source(\"helpers.R\")\nx <- 1\n")]);

    let map = entries(&db, files[0]);
    assert_eq!(map.len(), 1);
    assert_eq!(map.get("x"), Some(&vec![ExportEntry::Local]));
}

#[test]
fn test_source_call_to_resolved_file_produces_import_entries() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/helpers.R", "helper <- function() 1\n"),
        ("w/analysis.R", "source(\"helpers.R\")\nx <- 1\n"),
    ]);
    let helpers = files[0];
    let analysis = files[1];

    let map = entries(&db, analysis);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("x"), Some(&vec![ExportEntry::Local]));
    assert_eq!(
        map.get("helper"),
        Some(&vec![ExportEntry::Import {
            file: helpers,
            name: "helper".to_string(),
        }])
    );
}

#[test]
fn test_sourced_then_local_keep_both() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/helpers.R", "shared <- 1\n"),
        ("w/analysis.R", "source(\"helpers.R\")\nshared <- 2\n"),
    ]);
    let helpers = files[0];
    let analysis = files[1];

    // Multi-target keeps every candidate in definition order. The sourced
    // forward comes first, the local `shared <- 2` second. R's runtime takes
    // the last one (the local), but goto-def offers both.
    let map = entries(&db, analysis);
    assert_eq!(
        map.get("shared"),
        Some(&vec![
            ExportEntry::Import {
                file: helpers,
                name: "shared".to_string(),
            },
            ExportEntry::Local,
        ])
    );
}

#[test]
fn test_two_sources_of_same_name_keep_both_forwards() {
    let mut db = TestDb::new();

    // Both `b` and `c` define `dup`. R evaluates each `source()` in sequence
    // and the later one's assignment wins at runtime. Multi-target keeps both
    // forwards, in source order, so the runtime winner (`c`) is last.
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "dup <- 1\n"),
        ("w/c.R", "dup <- 2\n"),
        ("w/a.R", "source(\"b.R\")\nsource(\"c.R\")\n"),
    ]);
    let b = files[0];
    let c = files[1];
    let a = files[2];

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&vec![
            ExportEntry::Import {
                file: b,
                name: "dup".to_string(),
            },
            ExportEntry::Import {
                file: c,
                name: "dup".to_string(),
            },
        ])
    );
}

#[test]
fn test_sources_then_local_keep_all_in_order() {
    let mut db = TestDb::new();

    // sources first, then local. Local is the last assignment so it ends up
    // bound at end-of-file. Multi-target keeps both forwards plus the local, in
    // definition order, so the runtime winner (the local) is last.
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "dup <- 1\n"),
        ("w/c.R", "dup <- 2\n"),
        ("w/a.R", "source(\"b.R\")\nsource(\"c.R\")\ndup <- 3\n"),
    ]);
    let b = files[0];
    let c = files[1];
    let a = files[2];

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&vec![
            ExportEntry::Import {
                file: b,
                name: "dup".to_string(),
            },
            ExportEntry::Import {
                file: c,
                name: "dup".to_string(),
            },
            ExportEntry::Local,
        ])
    );
}

#[test]
fn test_source_local_source_keep_all_in_order() {
    let mut db = TestDb::new();

    // Local in the middle, sources either side. Matches R's runtime: each
    // statement assigns in order; the last write (the `c` source) wins.
    // Multi-target keeps all three candidates in definition order.
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "dup <- 1\n"),
        ("w/c.R", "dup <- 2\n"),
        ("w/a.R", "source(\"b.R\")\ndup <- 3\nsource(\"c.R\")\n"),
    ]);
    let b = files[0];
    let c = files[1];
    let a = files[2];

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&vec![
            ExportEntry::Import {
                file: b,
                name: "dup".to_string(),
            },
            ExportEntry::Local,
            ExportEntry::Import {
                file: c,
                name: "dup".to_string(),
            },
        ])
    );
}

#[test]
fn test_local_then_sources_keep_all_in_order() {
    let mut db = TestDb::new();

    // Local first, sources later. Sources reassign, last source wins at
    // runtime. Multi-target keeps all three candidates in definition order, so
    // the runtime winner (`c`) is last.
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "dup <- 1\n"),
        ("w/c.R", "dup <- 2\n"),
        ("w/a.R", "dup <- 3\nsource(\"b.R\")\nsource(\"c.R\")\n"),
    ]);
    let b = files[0];
    let c = files[1];
    let a = files[2];

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&vec![
            ExportEntry::Local,
            ExportEntry::Import {
                file: b,
                name: "dup".to_string(),
            },
            ExportEntry::Import {
                file: c,
                name: "dup".to_string(),
            },
        ])
    );
}

#[test]
fn test_repeated_local_moves_to_runtime_winning_position() {
    let mut db = TestDb::new();

    // Local, then a sourced forward of the same name, then a later local. The
    // two locals collapse to one `Local` marker, and the dedup keeps it at the
    // *last* local's position. So the entry order is `[Import{b}, Local]` and
    // the final entry is the binding R picks at runtime (`dup <- 3`), not the
    // earlier `Import{b}`.
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "dup <- 1\n"),
        ("w/a.R", "dup <- 2\nsource(\"b.R\")\ndup <- 3\n"),
    ]);
    let b = files[0];
    let a = files[1];

    let map = entries(&db, a);
    assert_eq!(
        map.get("dup"),
        Some(&vec![
            ExportEntry::Import {
                file: b,
                name: "dup".to_string(),
            },
            ExportEntry::Local,
        ])
    );
}

#[test]
fn test_source_call_inside_function_body_does_not_affect_top_level_exports() {
    let mut db = TestDb::new();

    // A function body's `source()` runs only when the function is
    // called; statically it injects into the function's runtime scope,
    // not into the file's top-level bindings.
    let files = setup_workspace(&mut db, &[
        ("w/helpers.R", "injected <- 1\n"),
        ("w/a.R", "f <- function() source(\"helpers.R\")\n"),
    ]);
    let a = files[1];

    let map = entries(&db, a);
    assert!(map.contains_key("f"));
    assert!(!map.contains_key("injected"));
}

#[test]
fn test_source_chain_forwards_through_two_files() {
    let mut db = TestDb::new();

    let files = setup_workspace(&mut db, &[
        ("w/leaf.R", "deep <- 1\n"),
        ("w/mid.R", "source(\"leaf.R\")\n"),
        ("w/top.R", "source(\"mid.R\")\n"),
    ]);
    let mid = files[1];
    let top = files[2];

    // top forwards `deep` through mid -> leaf. The forwarding entry on
    // top points to `mid` (the direct source target). Chasing the
    // chain via `File::resolve` lands on `leaf` (Local).
    let map = entries(&db, top);
    assert_eq!(
        map.get("deep"),
        Some(&vec![ExportEntry::Import {
            file: mid,
            name: "deep".to_string(),
        }])
    );
}

#[test]
fn test_cyclic_source_returns_empty_exports_without_panicking() {
    let mut db = TestDb::new();

    // a.R sources b.R; b.R sources a.R. Illegal user code. Both
    // `File::exports` and `File::semantic_index` carry cycle handlers;
    // `exports`'s FallbackImmediate returns empty for every cycle
    // participant. The test pins the observable behaviour at the
    // `exports` surface and that the call returns without panicking.
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "source(\"b.R\")\na_local <- 1\n"),
        ("w/b.R", "source(\"a.R\")\nb_local <- 2\n"),
    ]);
    let a = files[0];
    let b = files[1];

    let a_map = entries(&db, a);
    let b_map = entries(&db, b);
    assert!(a_map.is_empty());
    assert!(b_map.is_empty());
}

#[test]
fn test_editing_function_body_keeps_exports_stable() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "f <- function() 1\n")]);
    let file = files[0];

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
