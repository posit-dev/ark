use crate::test_path::file_path;
use crate::DbInputs;
use crate::FileRevision;
use crate::OakDatabase;

#[test]
fn test_intern_returns_existing_entity_unchanged_and_shares_with_snapshots() {
    let path = file_path("workspace/a.R");
    let description = file_path("workspace/DESCRIPTION");
    let mut db = OakDatabase::new();

    let file = db.intern_file(path.clone(), FileRevision::zero(), None);
    let pkg = db.intern_package(
        description.clone(),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
    );
    let before = salsa::plumbing::current_revision(&db);

    let new_revision = FileRevision::from(1);
    assert_eq!(db.intern_file(path.clone(), new_revision, Some(pkg)), file);
    assert_eq!(
        db.intern_package(
            description.clone(),
            "renamed".to_string(),
            new_revision,
            new_revision,
            Some(new_revision),
        ),
        pkg
    );

    assert_eq!(salsa::plumbing::current_revision(&db), before);
    assert_eq!(file.revision(&db), FileRevision::zero());
    assert_eq!(file.package(&db), None);
    assert_eq!(pkg.name(&db), "pkg");
    assert_eq!(pkg.index_revision(&db), None);

    let snapshot = db.snapshot();
    assert_eq!(snapshot.interned_file(&path), Some(file));
    assert_eq!(snapshot.interned_package(&description), Some(pkg));
}
