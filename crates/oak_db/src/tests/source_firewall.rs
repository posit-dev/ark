//! Body edits that preserve exports and attachments must not rebuild importers.
//!
//! Source resolution reads [`File::exports()`] and [`File::attached_packages()`].
//! When both summaries compare equal after an edit, Salsa preserves their
//! change timestamps and reuses the importer's cached [`File::semantic_index()`].
//! Changes to either summary must rebuild the importer.
//!
//! Per-file execution counts distinguish the target's necessary rebuild from
//! an unnecessary importer rebuild caused by reading the target's full index.

use salsa::Setter;

use crate::test_path::file_path;
use crate::tests::file_imports::install_packages;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;

const INDEX: &str = "File::semantic_index";

fn source_pair(helpers_text: &str) -> (TestDb, File, File) {
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr"]);
    let root = workspace_root(&db, "w");

    let main = File::new(
        &db,
        file_path("w/main.R"),
        FileRevision::zero(),
        Some("source(\"helpers.R\")\nf <- function() 1\n".to_string()),
        None,
    );
    let helpers = File::new(
        &db,
        file_path("w/helpers.R"),
        FileRevision::zero(),
        Some(helpers_text.to_string()),
        None,
    );

    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    (db, main, helpers)
}

fn warm(db: &TestDb, main: File) -> usize {
    let _ = main.semantic_index(db);
    db.executions_for(INDEX, main)
}

#[test]
fn test_body_edit_in_a_sourced_file_does_not_rebuild_the_importer_index() {
    let (mut db, main, helpers) = source_pair("y <- function() 1\n");
    let before = warm(&db, main);
    assert_eq!(before, 1);
    assert_eq!(db.executions_for(INDEX, helpers), 1);

    helpers
        .set_source_text_override(&mut db)
        .to(Some("y <- function() 2\n".to_string()));
    let _ = main.semantic_index(&db);

    assert_eq!(db.executions_for(INDEX, main), before);
    assert_eq!(db.executions_for(INDEX, helpers), 2);
}

#[test]
fn test_attach_preserving_body_edit_in_a_sourced_file_does_not_rebuild_the_importer_index() {
    let (mut db, main, helpers) = source_pair("library(dplyr)\ny <- function() 1\n");
    let before = warm(&db, main);

    helpers
        .set_source_text_override(&mut db)
        .to(Some("library(dplyr)\ny <- function() 2\n".to_string()));
    let _ = main.semantic_index(&db);

    assert_eq!(db.executions_for(INDEX, main), before);
    assert_eq!(db.executions_for(INDEX, helpers), 2);
}

#[test]
fn test_renaming_an_export_in_a_sourced_file_rebuilds_the_importer_index() {
    let (mut db, main, helpers) = source_pair("y <- function() 1\n");
    let before = warm(&db, main);

    helpers
        .set_source_text_override(&mut db)
        .to(Some("z <- function() 1\n".to_string()));
    let _ = main.semantic_index(&db);

    assert_eq!(db.executions_for(INDEX, main), before + 1);
}

#[test]
fn test_adding_an_attach_to_a_sourced_file_rebuilds_the_importer_index() {
    let (mut db, main, helpers) = source_pair("y <- function() 1\n");
    let before = warm(&db, main);

    helpers
        .set_source_text_override(&mut db)
        .to(Some("library(dplyr)\ny <- function() 1\n".to_string()));
    let _ = main.semantic_index(&db);

    assert_eq!(db.executions_for(INDEX, main), before + 1);
}
