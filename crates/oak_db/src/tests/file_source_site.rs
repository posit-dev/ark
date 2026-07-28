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
fn test_source_sites_is_stable_across_unrelated_edit() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/b.R", "x <- 1\n"),
        ("w/a.R", "source(\"b.R\")\n"),
    ]);
    let a = files[1];

    let before: Vec<SourceSite> = a.source_sites(&db).clone();
    assert_eq!(db.executions("source_sites"), 1);

    // Appending an unrelated statement changes a's semantic_index but not
    // its source() calls. There's no consumer of `source_sites` yet to
    // observe salsa backdating through, so this only pins that the returned
    // value itself is unchanged across the edit.
    a.set_source_text_override(&mut db)
        .to(Some("source(\"b.R\")\nx <- 1\n".to_string()));
    let after: Vec<SourceSite> = a.source_sites(&db).clone();

    assert_eq!(before, after);
}
