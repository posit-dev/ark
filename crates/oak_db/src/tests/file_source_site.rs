use oak_semantic::ScopeId;
use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::SourceSite;

/// Build a workspace root at `/w` populated with the given scripts.
/// Returns the file handles in the same order. Registers the root with
/// `WorkspaceRoots` so `file_by_path` finds the files.
fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "w");
    let files: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| {
            File::new(
                db,
                file_path(name),
                FileRevision::zero(),
                Some(contents.to_string()),
                None,
            )
        })
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    files
}

#[test]
fn test_source_call_to_registered_file_resolves_target() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "x <- 1\n"),
        ("w/a.R", "source(\"b.R\")\n"),
    ]);
    let b = files[0];
    let a = files[1];

    let sites = a.source_sites(&db);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].target(), Some(b));
    assert_eq!(sites[0].path(), "b.R");
    assert_eq!(sites[0].scope(), ScopeId::from(0));
}

#[test]
fn test_source_call_to_unregistered_path_keeps_site_with_no_target() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "source(\"nope.R\")\n")]);
    let a = files[0];

    let sites = a.source_sites(&db);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].target(), None);
    assert_eq!(sites[0].path(), "nope.R");
}

#[test]
fn test_two_source_calls_produce_sites_in_call_order() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "b_val <- 1\n"),
        ("w/c.R", "c_val <- 2\n"),
        ("w/a.R", "source(\"b.R\")\nsource(\"c.R\")\n"),
    ]);
    let a = files[2];

    let sites = a.source_sites(&db);
    assert_eq!(sites.len(), 2);
    assert_eq!(sites[0].path(), "b.R");
    assert_eq!(sites[1].path(), "c.R");
    assert!(sites[0].offset() < sites[1].offset());
}

#[test]
fn test_source_call_inside_function_body_has_non_file_scope() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/helpers.R", "helper <- 1\n"),
        ("w/a.R", "f <- function() source(\"helpers.R\")\n"),
    ]);
    let a = files[1];

    let sites = a.source_sites(&db);
    assert_eq!(sites.len(), 1);
    assert_ne!(sites[0].scope(), ScopeId::from(0));
}

#[test]
fn test_source_sites_yields_immediate_target_only() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/c.R", "c_val <- 1\n"),
        ("w/b.R", "source(\"c.R\")\n"),
        ("w/a.R", "source(\"b.R\")\n"),
    ]);
    let b = files[1];
    let a = files[2];

    // a sources b, b sources c. a's own semantic_calls() only records the
    // source() call literally in a's text, so c never shows up here.
    let sites = a.source_sites(&db);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].target(), Some(b));
    assert_eq!(sites[0].path(), "b.R");
}

#[test]
fn test_file_with_no_source_calls_has_empty_source_sites() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let a = files[0];

    assert!(a.source_sites(&db).is_empty());
}

#[test]
fn test_source_sites_backdates_across_unrelated_edit() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "x <- 1\n"),
        ("w/a.R", "source(\"b.R\")\n"),
    ]);
    let a = files[1];

    let before: Vec<SourceSite> = a.source_sites(&db).clone();
    let _ = a.sourced_by(&db);
    assert_eq!(db.executions("File::source_sites"), 2);
    assert_eq!(db.executions("sourcing_files_by_target"), 1);

    // Appending an unrelated statement changes a's semantic_index but not
    // its source() calls, so `source_sites` re-executes for `a` and returns
    // an unchanged value. `sourcing_files_by_target` reads that value, so it
    // backdates rather than re-executing.
    a.set_source_text_override(&mut db)
        .to(Some("source(\"b.R\")\nx <- 1\n".to_string()));
    let after: Vec<SourceSite> = a.source_sites(&db).clone();
    let _ = a.sourced_by(&db);

    assert_eq!(before, after);
    assert_eq!(db.executions("File::source_sites"), 3);
    assert_eq!(db.executions("sourcing_files_by_target"), 1);
}

#[test]
fn test_source_call_registers_reverse_site() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);
    let a = files[0];
    let b = files[1];

    assert_eq!(a.sourced_by(&db), &vec![b]);
}

#[test]
fn test_two_sourcing_files_are_ordered_by_path() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/c.R", "source(\"a.R\")\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);
    let a = files[0];
    let c = files[1];
    let b = files[2];

    assert_eq!(a.sourced_by(&db), &vec![b, c]);
}

#[test]
fn test_file_nobody_sources_is_sourced_by_nothing() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let a = files[0];

    assert!(a.sourced_by(&db).is_empty());
}

#[test]
fn test_unresolved_source_call_contributes_no_reverse_site() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "source(\"nope.R\")\n"),
    ]);
    let a = files[0];

    assert!(a.sourced_by(&db).is_empty());
}

#[test]
fn test_file_sourcing_same_target_twice_is_listed_once() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "source(\"a.R\")\nsource(\"a.R\")\n"),
    ]);
    let a = files[0];
    let b = files[1];

    assert_eq!(a.sourced_by(&db), &vec![b]);

    // Both call positions are still reachable, just through the volatile
    // forward query rather than the position-free reverse one.
    let sites = b.source_sites(&db);
    assert_eq!(sites.len(), 2);
    assert!(sites[0].offset() < sites[1].offset());
}

#[test]
fn test_file_sourcing_itself_does_not_panic_or_recurse() {
    // `source("a.R")` inside `a.R` cycles `semantic_index(a)` through
    // `exports(a)`, so the index gets rebuilt with `NoopImportsResolver`. That
    // resolver leaves `resolve_effects` at its default `None`, so a bare
    // `source()` isn't recognized as effectful and no site is recorded.
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "source(\"a.R\")\n")]);
    let a = files[0];

    assert!(a.sourced_by(&db).is_empty());
}

#[test]
fn test_adding_source_call_in_new_file_updates_sourced_by() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);
    let a = files[0];
    let b = files[1];
    assert_eq!(a.sourced_by(&db).len(), 1);

    let root = db.workspace_roots().roots(&db)[0];
    let c = File::new(
        &db,
        file_path("w/c.R"),
        FileRevision::zero(),
        Some("source(\"a.R\")\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b, c]);

    assert_eq!(a.sourced_by(&db), &vec![b, c]);
}

#[test]
fn test_edit_above_a_source_call_does_not_invalidate_sourced_by() {
    // Inserting a line above `source("a.R")` shifts its offset, so `b`'s
    // `source_sites` re-executes with a changed value. `sourced_by` carries no
    // offsets, so it stays green through an edit that changed no source edge.
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);
    let a = files[0];
    let b = files[1];

    let _ = a.sourced_by(&db);
    let before = b.source_sites(&db)[0].offset();
    assert_eq!(db.executions("File::sourced_by"), 1);

    b.set_source_text_override(&mut db)
        .to(Some("library(dplyr)\nsource(\"a.R\")\n".to_string()));

    let _ = a.sourced_by(&db);
    assert!(b.source_sites(&db)[0].offset() > before);
    assert_eq!(db.executions("File::sourced_by"), 1);
}

#[test]
fn test_sourcing_files_by_target_firewalls_file_additions_from_sourced_by() {
    // An unrelated body edit alone backdates all the way through (see the
    // previous test). Adding a file anywhere changes `workspace_files` for
    // real, forcing `sourcing_files_by_target` to re-execute, but `a`'s own
    // entry is unaffected so `File::sourced_by` still backdates.
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);
    let a = files[0];
    let b = files[1];

    let _ = a.sourced_by(&db);
    assert_eq!(db.executions("sourcing_files_by_target"), 1);
    assert_eq!(db.executions("File::sourced_by"), 1);

    let root = db.workspace_roots().roots(&db)[0];
    let elsewhere = File::new(
        &db,
        file_path("w/z.R"),
        FileRevision::zero(),
        Some("z_val <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b, elsewhere]);

    let _ = a.sourced_by(&db);
    assert_eq!(db.executions("sourcing_files_by_target"), 2);
    assert_eq!(db.executions("File::sourced_by"), 1);
}

#[test]
fn test_adding_source_call_to_existing_file_invalidates_sourced_by() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "a_val <- 1\n"), ("w/b.R", "x <- 1\n")]);
    let a = files[0];
    let b = files[1];

    assert!(a.sourced_by(&db).is_empty());
    assert_eq!(db.executions("File::sourced_by"), 1);

    b.set_source_text_override(&mut db)
        .to(Some("source(\"a.R\")\n".to_string()));

    assert_eq!(a.sourced_by(&db), &vec![b]);
    assert_eq!(db.executions("sourcing_files_by_target"), 2);
    assert_eq!(db.executions("File::sourced_by"), 2);
}

#[test]
fn test_removing_source_call_invalidates_sourced_by() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);
    let a = files[0];
    let b = files[1];

    assert_eq!(a.sourced_by(&db), &vec![b]);
    assert_eq!(db.executions("File::sourced_by"), 1);

    b.set_source_text_override(&mut db)
        .to(Some("x <- 1\n".to_string()));

    assert!(a.sourced_by(&db).is_empty());
    assert_eq!(db.executions("File::sourced_by"), 2);
}

#[test]
fn test_retargeting_source_call_moves_the_edge() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "a_val <- 1\n"),
        ("w/b.R", "b_val <- 2\n"),
        ("w/c.R", "source(\"a.R\")\n"),
    ]);
    let a = files[0];
    let b = files[1];
    let c = files[2];

    assert_eq!(a.sourced_by(&db), &vec![c]);
    assert!(b.sourced_by(&db).is_empty());

    c.set_source_text_override(&mut db)
        .to(Some("source(\"b.R\")\n".to_string()));

    assert!(a.sourced_by(&db).is_empty());
    assert_eq!(b.sourced_by(&db), &vec![c]);
}

#[test]
fn test_registering_the_target_resolves_a_dangling_source_call() {
    // `source_sites` resolves targets through `file_by_path`, so a site that
    // named a file the scanner hadn't reached yet fills in once it lands in a
    // root.
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/b.R", "source(\"a.R\")\n")]);
    let b = files[0];

    assert_eq!(b.source_sites(&db)[0].target(), None);

    let root = db.workspace_roots().roots(&db)[0];
    let a = File::new(
        &db,
        file_path("w/a.R"),
        FileRevision::zero(),
        Some("a_val <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);

    assert_eq!(b.source_sites(&db)[0].target(), Some(a));
    assert_eq!(a.sourced_by(&db), &vec![b]);
}
