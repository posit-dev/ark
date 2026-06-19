use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::ImportLayer;
use crate::Package;

/// Create a library root containing one installed package named `name`.
/// Returns the package and the root, both already wired up: the root's
/// `packages` is set to `[pkg]`. Callers register the returned roots on
/// `LibraryRoots` to make `package_by_name` see them.
fn make_installed(db: &mut TestDb, name: &str) -> (crate::Root, Package) {
    let root = library_root(db, &format!("libs/{name}"));
    let pkg = Package::new(
        db,
        file_path(&format!("libs/{name}/DESCRIPTION")),
        name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    root.set_packages(db).to(vec![pkg]);
    (root, pkg)
}

/// Register a set of installed packages on `LibraryRoots`. Replaces any
/// previously registered library roots.
fn install_packages(db: &mut TestDb, names: &[&str]) -> Vec<Package> {
    let mut roots = Vec::new();
    let mut packages = Vec::new();
    for &name in names {
        let (root, pkg) = make_installed(db, name);
        roots.push(root);
        packages.push(pkg);
    }
    db.library_roots().set_roots(db).to(roots);
    packages
}

#[test]
fn test_script_with_no_attaches_returns_only_default_search_path() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["base", "stats"]);
    let base = packages[0];
    let stats = packages[1];

    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );
    let layers = file.imports(&db);

    // Only `stats` and `base` are registered in this test; the other
    // default-search-path packages are absent and drop out.
    let packages: Vec<Package> = layers
        .iter()
        .map(|layer| match layer {
            ImportLayer::Package(p) => *p,
            other => panic!("unexpected layer: {other:?}"),
        })
        .collect();
    assert_eq!(packages, vec![stats, base]);
}

#[test]
fn test_script_attach_produces_package_exports_layer_in_lifo_order() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["dplyr", "ggplot2"]);
    let dplyr = packages[0];
    let ggplot2 = packages[1];

    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some("library(dplyr)\nlibrary(ggplot2)\n".to_string()),
        None,
    );
    let layers = file.imports(&db);

    let attached: Vec<Package> = layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::Package(p) => Some(*p),
            _ => None,
        })
        .collect();

    // LIFO: latest `library()` call comes first (matching R's runtime
    // search order). Then the default search path (empty in this setup).
    assert_eq!(attached, vec![ggplot2, dplyr]);
}

#[test]
fn test_script_attach_to_unregistered_package_drops_layer() {
    let db = TestDb::new();
    // No `dplyr` in any library root.
    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some("library(dplyr)\n".to_string()),
        None,
    );

    let layers = file.imports(&db);
    assert!(layers.is_empty());
}

#[test]
fn test_package_file_emits_namespace_and_collation_layers() {
    let mut db = TestDb::new();
    let installed = install_packages(&mut db, &["rlang", "base"]);
    let rlang = installed[0];

    let namespace = Namespace {
        imports: vec![Import {
            name: "abort".to_string(),
            package: "rlang".to_string(),
        }],
        package_imports: vec!["rlang".to_string()],
        ..Default::default()
    };

    // Build a workspace package with two R files. `Package.files`
    // holds them in declaration order; `package_layers` walks that
    // order to emit `File` layers.
    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let first = File::new(
        &db,
        file_path("w/pkg/R/_a.R"),
        FileRevision::zero(),
        Some("first <- 1\n".to_string()),
        Some(pkg),
    );
    let second = File::new(
        &db,
        file_path("w/pkg/R/b.R"),
        FileRevision::zero(),
        Some("second <- 2\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![first, second]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let layers = second.imports(&db);

    let mut shape = Vec::new();
    for layer in layers {
        match layer {
            ImportLayer::From(map) => {
                let mut entries: Vec<(String, String)> =
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                entries.sort();
                shape.push(format!("From({entries:?})"));
            },
            ImportLayer::Package(p) => {
                shape.push(format!("Package({})", p.name(&db)));
            },
            ImportLayer::File(f) => {
                let url = f.path(&db).to_url();
                shape.push(format!(
                    "File({})",
                    url.path().rsplit('/').next().unwrap_or("?")
                ));
            },
        }
    }

    let _ = rlang;
    assert_eq!(shape, vec![
        // Collation files first (R's package namespace looks at the
        // package's own bindings before its imports). Self (b.R) is
        // excluded: a file's own top-level bindings live in `exports`,
        // not `imports`.
        "File(_a.R)".to_string(),
        "From([(\"abort\", \"rlang\")])".to_string(),
        "Package(rlang)".to_string(),
        "Package(base)".to_string(),
    ]);
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

/// Render `ImportLayer`s to a stable, assertable shape. `File` layers
/// collapse to their basename.
fn shape(db: &TestDb, layers: &[ImportLayer]) -> Vec<String> {
    layers
        .iter()
        .map(|layer| match layer {
            ImportLayer::From(map) => {
                let mut entries: Vec<(String, String)> =
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                entries.sort();
                format!("From({entries:?})")
            },
            ImportLayer::Package(p) => format!("Package({})", p.name(db)),
            ImportLayer::File(f) => {
                let url = f.path(db).to_url();
                format!("File({})", url.path().rsplit('/').next().unwrap_or("?"))
            },
        })
        .collect()
}

#[test]
fn test_imports_is_cached_per_file() {
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["dplyr"]);

    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some("library(dplyr)\n".to_string()),
        None,
    );
    let _ = file.imports(&db);
    let _ = file.imports(&db);

    assert_eq!(db.executions("imports"), 1);
}
