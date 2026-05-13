use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::File;

#[test]
fn workspace_root_returns_none_for_orphan_file() {
    let db = TestDb::new();
    let file = File::new(&db, file_url("orphan.R"), String::new());

    assert_eq!(file.workspace_root(&db), None);
}

#[test]
fn workspace_root_finds_containing_workspace() {
    let mut db = TestDb::new();
    let workspace = workspace_root(&db, "proj");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let file = File::new(&db, file_url("proj/scripts/foo.R"), String::new());
    assert_eq!(file.workspace_root(&db), Some(workspace));
}

#[test]
fn workspace_root_returns_longest_prefix() {
    let mut db = TestDb::new();
    let outer = workspace_root(&db, "proj");
    let inner = workspace_root(&db, "proj/inner");
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![outer, inner]);

    let inner_file = File::new(&db, file_url("proj/inner/foo.R"), String::new());
    assert_eq!(inner_file.workspace_root(&db), Some(inner));

    let outer_file = File::new(&db, file_url("proj/foo.R"), String::new());
    assert_eq!(outer_file.workspace_root(&db), Some(outer));
}
