use biome_rowan::TextSize;
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
use crate::Name;
use crate::Package;

#[test]
fn test_testthat_environment_chain_separates_tests_support_and_namespace() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
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
    let sources = [
        ("w/pkg/R/a.R", "x <- 0\nf <- function() test_only\n"),
        (
            "w/pkg/tests/testthat/helper-a.R",
            "if (first) x <- 1\nx\nf <- function() x\n",
        ),
        ("w/pkg/tests/testthat/helper-b.R", "if (second) x <- 2\n"),
        (
            "w/pkg/tests/testthat/test-a.R",
            "x <- 3\ntest_only <- 1\nf <- function() x\n",
        ),
        (
            "w/pkg/tests/testthat/test-b.R",
            "f <- function() test_only\n",
        ),
    ];
    let files: Vec<File> = sources
        .iter()
        .map(|(path, source)| {
            File::new(
                &db,
                file_path(path),
                FileRevision::zero(),
                Some(source.to_string()),
                Some(pkg),
            )
        })
        .collect();
    pkg.set_files(&mut db).to(vec![files[0]]);
    pkg.set_scripts(&mut db).to(files[1..].to_vec());
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let name = Name::new(&db, "x");
    let helper = files[1];
    let deferred = helper.resolve(&db, name);
    assert_eq!(
        deferred.iter().map(|def| def.file(&db)).collect::<Vec<_>>(),
        vec![files[2], helper, files[0]]
    );
    let lazy_offset = TextSize::from(sources[1].1.rfind('x').unwrap() as u32);
    assert_eq!(helper.resolve_at(&db, lazy_offset), deferred);

    let eager_offset = TextSize::from((sources[1].1.find("\nx\n").unwrap() + 1) as u32);
    let eager = helper.resolve_at(&db, eager_offset);
    assert_eq!(
        eager.iter().map(|def| def.file(&db)).collect::<Vec<_>>(),
        vec![helper, files[0]]
    );

    let test = files[3].resolve(&db, name);
    assert_eq!(
        test.iter().map(|def| def.file(&db)).collect::<Vec<_>>(),
        vec![files[3]]
    );
    for file in [files[0], files[1], files[4]] {
        assert!(file.resolve(&db, Name::new(&db, "test_only")).is_empty());
    }
}

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

#[test]
fn test_helper_backward_source_into_setup_cycles() {
    // `helper*.R` sorts before `setup*.R`, so `setup.R` is `helper.R`'s
    // collation successor, but `helper.R` also sources it explicitly. Same
    // shape and same outcome as the Shiny and package `R/` cases: resolving
    // `setup.R`'s own `library(pkgb)` reads `helper.R`'s `attached_packages`
    // as a support-file predecessor, which cycles back through
    // `helper.R`'s own `source()` resolution. Both files degrade, both
    // attaches are lost, and both carry the same `SourceCycle` diagnostic
    // even though `setup.R` has no `source()` call of its own.
    let mut db = TestDb::new();
    install_packages(&mut db, &["testthat", "base", "pkga", "pkgb"]);

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
    let helper = File::new(
        &db,
        file_path("w/pkg/tests/testthat/helper.R"),
        FileRevision::zero(),
        Some("library(pkga)\nsource(\"pkg/tests/testthat/setup.R\")\n".to_string()),
        Some(pkg),
    );
    let setup = File::new(
        &db,
        file_path("w/pkg/tests/testthat/setup.R"),
        FileRevision::zero(),
        Some("library(pkgb)\n".to_string()),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![helper, setup]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(shape(&db, helper.imports(&db)), vec![
        "File(setup.R)".to_string(),
        "Package(testthat)".to_string(),
        "Package(base)".to_string(),
    ]);
    assert_eq!(shape(&db, setup.imports(&db)), vec![
        "File(helper.R)".to_string(),
        "Package(testthat)".to_string(),
        "Package(base)".to_string(),
    ]);
    assert!(helper.sourced_by(&db).is_empty());
    assert!(setup.sourced_by(&db).is_empty());

    assert_eq!(helper.diagnostics(&db).len(), 1);
    assert_eq!(setup.diagnostics(&db).len(), 1);
    assert_eq!(
        helper.diagnostics(&db)[0].message(),
        setup.diagnostics(&db)[0].message()
    );
}
