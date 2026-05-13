use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::Script;
use crate::SourceNode;

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
fn intern_file_updates_parent_on_re_intern() {
    // The Vfs pattern: create the file orphan, then create the Script
    // wrapping it, then re-intern with the back-pointer set.
    let mut db = TestDb::new();
    let url = file_url("a.R");

    let file = intern_file(&mut db, url.clone(), "x <- 1\n".to_string(), None);
    assert_eq!(file.parent(&db), None);

    let script = Script::new(&db, file);
    file.set_parent(&mut db)
        .to(Some(SourceNode::Script(script)));
    assert_eq!(file.parent(&db), Some(SourceNode::Script(script)));
}

#[test]
fn script_by_url_uses_files_interner() {
    // After PR 7's collapse, `script_by_url` is O(1): it reads the
    // `File` from `Files` by URL and matches on `File.parent`.
    let mut db = TestDb::new();
    let url = file_url("a.R");
    let file = intern_file(&mut db, url.clone(), String::new(), None);
    let script = Script::new(&db, file);
    file.set_parent(&mut db)
        .to(Some(SourceNode::Script(script)));

    // No `SourceGraph::set_scripts` call: the lookup goes through
    // `Files` + `File.parent`, not through `SourceGraph.scripts`.
    assert_eq!(db.source_graph().script_by_url(&db, &url), Some(script));
}

#[test]
fn script_by_url_ignores_package_files() {
    use oak_package_metadata::namespace::Namespace;

    use crate::tests::test_db::workspace_root;
    use crate::Package;
    use crate::PackageOrigin;

    let mut db = TestDb::new();
    let url = file_url("R/utils.R");
    let file = intern_file(&mut db, url.clone(), String::new(), None);
    let pkg = Package::new(
        &db,
        "mypkg".to_string(),
        PackageOrigin::Workspace {
            root: workspace_root(&db, "workspace/mypkg"),
        },
        Namespace::default(),
        vec![file],
    );
    file.set_parent(&mut db).to(Some(SourceNode::Package(pkg)));

    // Package files are not scripts.
    assert_eq!(db.source_graph().script_by_url(&db, &url), None);
}
