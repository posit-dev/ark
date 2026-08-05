use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::file_imports::install_packages;
use crate::tests::file_imports::shape;
use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Package;

#[test]
fn test_testthat_file_sees_helpers_package_and_testthat() {
    let mut db = TestDb::new();
    let installed = install_packages(&mut db, &["testthat", "base"]);
    let testthat = installed[0];
    let base = installed[1];

    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );

    let r_file = File::new(
        &db,
        file_path("w/pkg/R/a.R"),
        FileRevision::zero(),
        Some("f <- 1\n".to_string()),
        Some(pkg),
    );
    let helper = File::new(
        &db,
        file_path("w/pkg/tests/testthat/helper-b.R"),
        FileRevision::zero(),
        Some("h <- 1\n".to_string()),
        Some(pkg),
    );
    let setup = File::new(
        &db,
        file_path("w/pkg/tests/testthat/setup-c.R"),
        FileRevision::zero(),
        Some("s <- 1\n".to_string()),
        Some(pkg),
    );
    let test_foo = File::new(
        &db,
        file_path("w/pkg/tests/testthat/test-foo.R"),
        FileRevision::zero(),
        Some("test_that('x', expect_true(TRUE))\n".to_string()),
        Some(pkg),
    );
    // A sibling test file. Each test file runs in its own environment, so
    // it must not appear in `test_foo`'s imports.
    let test_bar = File::new(
        &db,
        file_path("w/pkg/tests/testthat/test-bar.R"),
        FileRevision::zero(),
        Some("test_that('y', expect_true(TRUE))\n".to_string()),
        Some(pkg),
    );

    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db)
        .to(vec![helper, setup, test_foo, test_bar]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let _ = (testthat, base);
    assert_eq!(shape(&db, test_foo.imports(&db)), vec![
        // helper/setup files come first (sourced into the test env). LIFO
        // over byte-order basename sort, so `setup-c` (sourced last)
        // outranks `helper-b`.
        "File(setup-c.R)".to_string(),
        "File(helper-b.R)".to_string(),
        // Then the package's own R/ code.
        "File(a.R)".to_string(),
        // testthat is attached, base is always last.
        "Package(testthat)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_package_r_file_does_not_take_testthat_path() {
    let mut db = TestDb::new();
    let installed = install_packages(&mut db, &["testthat", "base"]);
    let base = installed[1];

    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    let r_file = File::new(
        &db,
        file_path("w/pkg/R/a.R"),
        FileRevision::zero(),
        Some("f <- 1\n".to_string()),
        Some(pkg),
    );
    let helper = File::new(
        &db,
        file_path("w/pkg/tests/testthat/helper-b.R"),
        FileRevision::zero(),
        Some("h <- 1\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db).to(vec![helper]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let _ = base;
    // An `R/` file is not a testthat file: no helper layer, no testthat
    // layer, just base (no other R/ files, empty namespace).
    assert_eq!(shape(&db, r_file.imports(&db)), vec![
        "Package(base)".to_string()
    ]);
}

#[test]
fn test_testthat_file_includes_top_level_library_calls() {
    let mut db = TestDb::new();
    let installed = install_packages(&mut db, &["cli", "testthat", "base"]);
    let cli = installed[0];
    let testthat = installed[1];
    let base = installed[2];

    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    let r_file = File::new(
        &db,
        file_path("w/pkg/R/a.R"),
        FileRevision::zero(),
        Some("f <- 1\n".to_string()),
        Some(pkg),
    );
    let test_foo = File::new(
        &db,
        file_path("w/pkg/tests/testthat/test-foo.R"),
        FileRevision::zero(),
        Some("library(cli)\ntest_that('x', expect_true(TRUE))\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db).to(vec![test_foo]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let _ = (cli, testthat, base);
    assert_eq!(shape(&db, test_foo.imports(&db)), vec![
        // The package's own R/ code.
        "File(a.R)".to_string(),
        // The test file's own `library()` call sits below the package but
        // above testthat (attached more recently than the runner attached
        // testthat).
        "Package(cli)".to_string(),
        "Package(testthat)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_testthat_file_includes_package_namespace_imports() {
    // A test file runs under the package namespace, so the package's
    // `importFrom(rlang, abort)` shows up as a `From` layer, ranked above the
    // implicit testthat/base attaches.
    let mut db = TestDb::new();
    install_packages(&mut db, &["testthat", "base"]);
    let workspace = workspace_root(&db, "w");
    let namespace = Namespace {
        imports: vec![Import {
            name: "abort".to_string(),
            package: "rlang".to_string(),
        }],
        ..Default::default()
    };
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let test_foo = File::new(
        &db,
        file_path("w/pkg/tests/testthat/test-foo.R"),
        FileRevision::zero(),
        Some("test_that('x', expect_true(TRUE))\n".to_string()),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![test_foo]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(shape(&db, test_foo.imports(&db)), vec![
        "From([(\"abort\", \"rlang\")])".to_string(),
        "Package(testthat)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_testthat_file_ignores_source_sites() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["testthat", "base"]);

    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    let r_file = File::new(
        &db,
        file_path("w/pkg/R/a.R"),
        FileRevision::zero(),
        Some("f <- 1\n".to_string()),
        Some(pkg),
    );
    let helper = File::new(
        &db,
        file_path("w/pkg/tests/testthat/helper-b.R"),
        FileRevision::zero(),
        Some("h <- 1\n".to_string()),
        Some(pkg),
    );
    let test_foo = File::new(
        &db,
        file_path("w/pkg/tests/testthat/test-foo.R"),
        FileRevision::zero(),
        Some("source(\"pkg/tests/testthat/helper-b.R\")\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db).to(vec![helper, test_foo]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    // testthat sources helpers itself, before any test file runs, so an
    // explicit `source()` in a test file doesn't change what the helper sees.
    assert_eq!(helper.sourced_by(&db), &vec![test_foo]);
    assert_eq!(shape(&db, helper.imports(&db)), vec![
        "File(a.R)".to_string(),
        "Package(testthat)".to_string(),
        "Package(base)".to_string(),
    ]);
}
