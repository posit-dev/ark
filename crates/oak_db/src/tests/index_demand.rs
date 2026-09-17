//! Verifies which file-keyed queries demand the named file's semantic index.
//!
//! The fuzz mutator uses [`crate::fuzz::choose::observed_file()`] to pair edits
//! with direct observers. These execution counts keep that classification
//! aligned with the query dependencies. [`File::cross_file_layers()`] is the
//! counterexample: it can name a file without reading its program.

use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::file_imports::CollationView;
use crate::test_path::file_path;
use crate::tests::file_imports::install_packages;
use crate::tests::test_db::make_package;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Name;

const INDEX: &str = "File::semantic_index";

const TEXT: &str = "library(base)\nsource(\"other.R\")\nval <- 1\n";

/// Uses one file so no other workspace file can trigger its index.
fn script() -> (TestDb, File) {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let root = workspace_root(&db, "w");
    let file = File::new(
        &db,
        file_path("w/main.R"),
        FileRevision::zero(),
        Some(TEXT.to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);
    (db, file)
}

/// A single-file package lets [`File::cross_file_layers()`] answer from load
/// context without demanding the file's index.
fn package_file() -> (TestDb, File) {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let (pkg, files) = make_package(&mut db, "mypkg", Namespace::default(), &[(
        "ws/mypkg/R/a.R",
        TEXT,
    )]);
    let root = workspace_root(&db, "ws/mypkg");
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);
    (db, files[0])
}

fn eof() -> TextSize {
    TextSize::from(TEXT.len() as u32)
}

/// Each direct observer must execute the named file's index. A fresh database
/// prevents earlier queries from warming it.
#[test]
fn test_direct_observers_demand_the_files_index() {
    for fixture in [script as fn() -> (TestDb, File), package_file] {
        let (db, file) = fixture();
        let _ = file.diagnostics(&db);
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.imports(&db);
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.imports_at(&db, eof());
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.resolve_at(&db, eof());
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.resolve(&db, Name::new(&db, "val"));
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.used_packages(&db);
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.semantic_index(&db);
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.exports(&db);
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.attached_packages(&db);
        assert_eq!(db.executions_for(INDEX, file), 1);

        let (db, file) = fixture();
        let _ = file.attached_packages_anywhere(&db);
        assert_eq!(db.executions_for(INDEX, file), 1);
    }
}

/// Pairing an edit with an observer is useful only if the invalidated index
/// executes again.
#[test]
fn test_an_observer_rebuilds_the_index_after_a_replacement() {
    let (mut db, file) = script();
    let _ = file.diagnostics(&db);
    assert_eq!(db.executions_for(INDEX, file), 1);

    file.set_source_text_override(&mut db)
        .to(Some("other <- function() 1\n".to_string()));
    let _ = file.diagnostics(&db);

    assert_eq!(db.executions_for(INDEX, file), 2);
}

/// [`File::cross_file_layers()`] names the file but does not demand its index in
/// a single-file package.
#[test]
fn test_cross_file_layers_does_not_demand_a_single_file_packages_index() {
    let (db, file) = package_file();
    let _ = file.cross_file_layers(&db, CollationView::Eager);

    assert_eq!(db.executions_for(INDEX, file), 0);
}
