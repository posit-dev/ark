use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::collation_files;
use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::File;
use crate::Package;
use crate::PackageOrigin;
use crate::Root;
use crate::SourceNode;

fn package(db: &TestDb, name: &str, root: Root, collation: Option<Vec<String>>) -> Package {
    Package::new(
        db,
        name.to_string(),
        PackageOrigin::Workspace { root },
        Namespace::default(),
        collation,
    )
}

fn intern_package_file(db: &mut TestDb, path: &str, pkg: Package) -> File {
    intern_file(
        db,
        file_url(path),
        String::new(),
        Some(SourceNode::Package(pkg)),
    )
}

#[test]
fn alphabetical_default_orders_by_basename() {
    // No `Collate` field: `collation_files` walks `Files::entries()`,
    // filters to direct children of `<root>/R/`, and sorts.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "pkg");
    let pkg = package(&db, "pkg", root, None);

    // Intern in non-alphabetical order to confirm the sort applies.
    let c = intern_package_file(&mut db, "/pkg/R/c.R", pkg);
    let a = intern_package_file(&mut db, "/pkg/R/a.R", pkg);
    let b = intern_package_file(&mut db, "/pkg/R/b.R", pkg);

    assert_eq!(collation_files(&db, pkg), &vec![a, b, c]);
}

#[test]
fn spec_preserves_declared_order() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "pkg");
    // `Collate` field puts c before a, against alphabetical order.
    let pkg = package(
        &db,
        "pkg",
        root,
        Some(vec!["c.R".to_string(), "a.R".to_string()]),
    );

    let a = intern_package_file(&mut db, "/pkg/R/a.R", pkg);
    let c = intern_package_file(&mut db, "/pkg/R/c.R", pkg);

    assert_eq!(collation_files(&db, pkg), &vec![c, a]);
}

#[test]
fn spec_drops_basenames_not_yet_interned() {
    // Vfs scan can populate the spec from `DESCRIPTION` before the
    // corresponding `R/` files have been interned. Missing entries
    // fall out; downstream queries see the partial list rather than
    // a broken reference.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "pkg");
    let pkg = package(
        &db,
        "pkg",
        root,
        Some(vec![
            "a.R".to_string(),
            "missing.R".to_string(),
            "b.R".to_string(),
        ]),
    );

    let a = intern_package_file(&mut db, "/pkg/R/a.R", pkg);
    let b = intern_package_file(&mut db, "/pkg/R/b.R", pkg);

    assert_eq!(collation_files(&db, pkg), &vec![a, b]);
}

#[test]
fn alphabetical_ignores_subdirectories() {
    // Only direct `R/` children participate. Subdirectories like
    // `R/man/`, test fixtures elsewhere in the workspace, etc are
    // filtered out.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "pkg");
    let pkg = package(&db, "pkg", root, None);

    let a = intern_package_file(&mut db, "/pkg/R/a.R", pkg);
    let _nested = intern_package_file(&mut db, "/pkg/R/man/helpers.R", pkg);
    let _sibling = intern_package_file(&mut db, "/pkg/other.R", pkg);

    assert_eq!(collation_files(&db, pkg), &vec![a]);
}

#[test]
fn installed_package_has_no_collation_files() {
    // Installed packages don't ship sources through workspace `R/`, so
    // `collation_files` returns empty even when an explicit spec is
    // set on the input.
    let db = TestDb::new();
    let pkg = Package::new(
        &db,
        "dplyr".to_string(),
        PackageOrigin::Installed {
            version: "1.0.0".to_string(),
            libpath: file_url("libs/dplyr"),
        },
        Namespace::default(),
        Some(vec!["a.R".to_string()]),
    );

    assert!(collation_files(&db, pkg).is_empty());
}

#[test]
fn revision_bump_invalidates_cached_result() {
    // The query reads `root.revision(db)`, so bumping it forces salsa
    // to re-run on the next read. This is the hook Vfs `update_file`
    // / `remove_file` will use to invalidate after mutating `Files`.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "pkg");
    let pkg = package(&db, "pkg", root, None);

    let a = intern_package_file(&mut db, "/pkg/R/a.R", pkg);
    assert_eq!(collation_files(&db, pkg), &vec![a]);

    // Intern a new file. Without a revision bump, the cached result
    // stays stale because `Files::intern` doesn't trip any salsa
    // dependency.
    let b = intern_package_file(&mut db, "/pkg/R/b.R", pkg);
    root.set_revision(&mut db).to(1);
    assert_eq!(collation_files(&db, pkg), &vec![a, b]);
}
