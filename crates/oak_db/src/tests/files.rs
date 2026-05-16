use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Package;

#[test]
fn intern_file_is_idempotent_on_same_url() {
    let mut db = TestDb::new();
    let url = file_url("a.R");

    let first = intern_file(&mut db, url.clone(), "x <- 1\n".to_string(), None);
    let second = intern_file(&mut db, url.clone(), "x <- 2\n".to_string(), None);

    // Same URL: same `File` salsa entity, contents updated via `set_*`.
    assert_eq!(first, second);
    assert_eq!(first.contents(&db), "x <- 2\n");
}

#[test]
fn intern_file_updates_package_on_re_intern() {
    // The Vfs pattern: create the file orphan, then create the Package
    // wrapping it, then re-intern with the back-pointer set.
    let mut db = TestDb::new();
    let url = file_url("R/utils.R");
    let root = workspace_root(&db, "workspace/mypkg");

    let file = intern_file(&mut db, url.clone(), "x <- 1\n".to_string(), None);
    assert_eq!(file.package(&db), None);

    let pkg = Package::new(
        &db,
        root,
        "mypkg".to_string(),
        None,
        Namespace::default(),
        vec![file],
        None,
    );
    file.set_package(&mut db).to(Some(pkg));
    assert_eq!(file.package(&db), Some(pkg));
}
