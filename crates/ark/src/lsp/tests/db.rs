use aether_path::FilePath;
use oak_db::File;
use oak_db::OakDatabase;
use url::Url;

use crate::lsp::db::FileArkExt;

fn file(db: &OakDatabase, contents: &str) -> File {
    let url = FilePath::from_url(&Url::parse("file:///test.R").unwrap());
    File::new(db, url, contents.to_string(), None)
}

#[test]
fn test_tree_sitter_parses_contents() {
    let db = OakDatabase::new();
    let file = file(&db, "x <- 1\n");

    let root = file.tree_sitter(&db).root_node();

    assert!(!root.has_error());
    assert_eq!(root.end_byte(), "x <- 1\n".len());
}

#[test]
fn test_tree_sitter_is_cached() {
    let db = OakDatabase::new();
    let file = file(&db, "x <- 1\n");

    // `returns(ref)` hands back the same memoized tree on repeat calls.
    let first = file.tree_sitter(&db);
    let second = file.tree_sitter(&db);
    assert!(std::ptr::eq(first, second));
}
