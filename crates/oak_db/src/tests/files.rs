use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::FileOwner;
use crate::Script;

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
fn intern_file_updates_owner_on_re_intern() {
    // The Vfs pattern: create the file orphan, then create the Script
    // wrapping it, then re-intern with the back-pointer set.
    let mut db = TestDb::new();
    let url = file_url("a.R");
    let root = workspace_root(&db, "");

    let file = intern_file(&mut db, url.clone(), "x <- 1\n".to_string(), None);
    assert_eq!(file.owner(&db), None);

    let script = Script::new(&db, root, file);
    file.set_owner(&mut db).to(Some(FileOwner::Script(script)));
    assert_eq!(file.owner(&db), Some(FileOwner::Script(script)));
}

#[test]
fn script_by_url_uses_files_interner() {
    // PR 8: `script_by_url` is O(1): it reads the `File` from `Files`
    // by URL and matches on `File.owner`.
    let mut db = TestDb::new();
    let url = file_url("a.R");
    let root = workspace_root(&db, "");
    let file = intern_file(&mut db, url.clone(), String::new(), None);
    let script = Script::new(&db, root, file);
    file.set_owner(&mut db).to(Some(FileOwner::Script(script)));

    // No `Root::set_scripts` call: the lookup goes through `Files` +
    // `File.owner`, not through `Root.scripts`.
    assert_eq!(db.files().get_script(&db, &url), Some(script));
}

#[test]
fn script_by_url_ignores_package_files() {
    use oak_package_metadata::namespace::Namespace;

    use crate::Package;

    let mut db = TestDb::new();
    let url = file_url("R/utils.R");
    let root = workspace_root(&db, "workspace/mypkg");
    let file = intern_file(&mut db, url.clone(), String::new(), None);
    let pkg = Package::new(
        &db,
        root,
        "mypkg".to_string(),
        None,
        Namespace::default(),
        vec![file],
        None,
    );
    file.set_owner(&mut db).to(Some(FileOwner::Package(pkg)));

    // Package files are not scripts.
    assert_eq!(db.files().get_script(&db, &url), None);
}
