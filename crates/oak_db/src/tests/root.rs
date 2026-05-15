use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;

#[test]
fn root_by_url_returns_none_with_no_workspace_roots() {
    let db = TestDb::new();
    assert_eq!(db.root_by_url( &file_url("workspace/foo.R")), None);
}

#[test]
fn root_by_url_finds_containing_root() {
    let mut db = TestDb::new();
    let workspace = workspace_root(&db, "workspace");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(
        db.root_by_url( &file_url("workspace/scripts/foo.R")),
        Some(workspace),
    );
}

#[test]
fn root_by_url_longest_prefix_wins() {
    let mut db = TestDb::new();
    let outer = workspace_root(&db, "proj");
    let inner = workspace_root(&db, "proj/inner");
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![outer, inner]);

    // File inside the inner root: longest-prefix wins.
    assert_eq!(db.root_by_url( &file_url("proj/inner/foo.R")), Some(inner),);
    // File only inside the outer root.
    assert_eq!(db.root_by_url( &file_url("proj/foo.R")), Some(outer));
}

#[test]
fn root_by_url_returns_none_when_url_lies_outside_every_root() {
    let mut db = TestDb::new();
    let workspace = workspace_root(&db, "workspace");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(db.root_by_url( &file_url("elsewhere/foo.R")), None);
}
