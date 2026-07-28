use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::make_package;
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
    install_packages(&mut db, &[
        "stats",
        "graphics",
        "grDevices",
        "utils",
        "datasets",
        "methods",
        "base",
    ]);

    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );

    // The whole default search path shows up as `Package` layers in R's
    // `search()` order (`stats` first, `base` last).
    assert_eq!(shape(&db, file.imports(&db)), vec![
        "Package(stats)".to_string(),
        "Package(graphics)".to_string(),
        "Package(grDevices)".to_string(),
        "Package(utils)".to_string(),
        "Package(datasets)".to_string(),
        "Package(methods)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_script_attach_produces_package_exports_layer_in_lifo_order() {
    let mut db = TestDb::new();
    // Only the attached packages are installed, so the default search path
    // drops out and the assertion sees just the two `library()` calls.
    install_packages(&mut db, &["dplyr", "ggplot2"]);

    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some("library(dplyr)\nlibrary(ggplot2)\n".to_string()),
        None,
    );

    // LIFO: latest `library()` call comes first (matching R's runtime search
    // order).
    assert_eq!(shape(&db, file.imports(&db)), vec![
        "Package(ggplot2)".to_string(),
        "Package(dplyr)".to_string(),
    ]);
}

#[test]
fn test_script_attach_to_unregistered_package_drops_layer() {
    let db = TestDb::new();
    // No `dplyr` (or default search path) in any root, so nothing has a
    // `Package` entity and every layer drops out.
    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some("library(dplyr)\n".to_string()),
        None,
    );

    assert!(file.imports(&db).is_empty());
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
        None,
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
            ImportLayer::From(package) => {
                let mut entries: Vec<(String, String)> = package
                    .imported_from(&db)
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
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
fn test_package_script_resolves_as_standalone_script() {
    // A `data-raw/` file carries a package back-pointer but lives in
    // `scripts`, not `files`. It isn't loaded with the package, so its
    // imports must be the standalone-script view, never the package view
    // (no `R/` `File` layers, no namespace layers). See #1270.
    let mut db = TestDb::new();
    let installed = install_packages(&mut db, &["dplyr", "base"]);
    let dplyr = installed[0];

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
        Some("internal <- 1\n".to_string()),
        Some(pkg),
    );
    let data_raw = File::new(
        &db,
        file_path("w/pkg/data-raw/prep.R"),
        FileRevision::zero(),
        Some("library(dplyr)\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db).to(vec![data_raw]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let _ = dplyr;
    // Its own `library(dplyr)` on top of the default search path (only `base`
    // installed here). `R/a.R` and the namespace are absent because the script
    // isn't part of the package's namespace.
    assert_eq!(shape(&db, data_raw.imports(&db)), vec![
        "Package(dplyr)".to_string(),
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
fn test_package_file_includes_predecessor_attaches() {
    // A collation predecessor's own `library(dep)` puts dep on the search path
    // for files loaded after it, so `b.R`'s imports carry it as a `Package`
    // layer, below the predecessor's `File` layer and above `base`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["dep", "base"]);
    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(Namespace::default()),
        Vec::new(),
        Vec::new(),
    );
    let a = File::new(
        &db,
        file_path("w/pkg/R/a.R"),
        FileRevision::zero(),
        Some("library(dep)\n".to_string()),
        Some(pkg),
    );
    let b = File::new(
        &db,
        file_path("w/pkg/R/b.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![a, b]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(shape(&db, b.imports(&db)), vec![
        "File(a.R)".to_string(),
        "Package(dep)".to_string(),
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

/// Render `ImportLayer`s to a stable, assertable shape. `File` layers
/// collapse to their basename.
fn shape(db: &TestDb, layers: &[ImportLayer]) -> Vec<String> {
    layers
        .iter()
        .map(|layer| match layer {
            ImportLayer::From(package) => {
                let mut entries: Vec<(String, String)> = package
                    .imported_from(db)
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
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

#[test]
fn test_cross_file_layers_cascade_stays_shallow() {
    // Building a late-collation file resolves its effects, which reaches back
    // through every predecessor's index. Without the forward-prime in
    // `predecessor_attach_layers` that walk recurses as deep as the collation,
    // which is what overflowed the stack in the IDE. Assert the nesting stays
    // flat regardless of collation length.
    //
    // We assert on recursion depth, not "does it crash", because
    // `stacker::maybe_grow` in `build_semantic_index` grows the stack and would
    // hide a regression from a crash-based check. Depth is independent of it.
    let mut db = TestDb::new();

    const N: usize = 150;
    let owned: Vec<(String, String)> = (0..N)
        .map(|i| (format!("w/pkg/R/f{i:04}.R"), "local(1)\n".to_string()))
        .collect();
    let files: Vec<(&str, &str)> = owned
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect();
    let (_pkg, entities) = make_package(&mut db, "pkg", Namespace::default(), &files);

    crate::file::recursion_depth::reset();

    // Cold build of the last collation file: its effect resolution demands
    // every predecessor's index. Forward-priming keeps the nesting shallow.
    let _ = entities[N - 1].semantic_index(&db);

    assert!(crate::file::recursion_depth::max() < 10);
}

#[test]
fn test_cross_file_layers_memoized_across_effect_calls() {
    // Each effectful call consults `cross_file_layers(file, view)` while the
    // file's index builds. Memoizing it keeps that to one execution per file
    // instead of one per call, which was the O(N^2) that froze the LSP.
    let mut db = TestDb::new();

    let body = "local(1)\n".repeat(20);
    let (_pkg, entities) = make_package(&mut db, "pkg", Namespace::default(), &[(
        "w/pkg/R/a.R",
        &body,
    )]);

    let _ = entities[0].semantic_index(&db);

    assert_eq!(db.executions("cross_file_layers"), 1);
}

#[test]
fn test_script_r_directory_siblings_see_each_other() {
    // Non-package scripts in an `R/` directory are collated alphabetically,
    // exactly like a package `R/` with no `Collate:` (#15144, #14790).
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/R/a.R"),
        FileRevision::zero(),
        Some("a_val <- 1\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("ws/R/b.R"),
        FileRevision::zero(),
        Some("b_val <- 2\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, a.imports(&db)), vec!["File(b.R)".to_string()]);
    assert_eq!(shape(&db, b.imports(&db)), vec!["File(a.R)".to_string()]);
}

#[test]
fn test_script_outside_r_directory_stays_standalone() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/scripts/a.R"),
        FileRevision::zero(),
        Some("a_val <- 1\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("ws/scripts/b.R"),
        FileRevision::zero(),
        Some("b_val <- 2\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, a.imports(&db)), Vec::<String>::new());
    assert_eq!(shape(&db, b.imports(&db)), Vec::<String>::new());
}

#[test]
fn test_package_owned_r_file_excluded_from_collate_stays_standalone() {
    // An `R/` file left out of `Collate:` carries a package back-pointer but
    // isn't in `package.files()`, so it must keep resolving as a standalone
    // script (same as `data-raw/`) and not take the new script-collation arm
    // just because it sits in an `R/` directory. That arm is gated on
    // `package(db) == None`.
    let mut db = TestDb::new();
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
        Some("internal <- 1\n".to_string()),
        Some(pkg),
    );
    let extra = File::new(
        &db,
        file_path("w/pkg/R/extra.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db).to(vec![extra]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(shape(&db, extra.imports(&db)), Vec::<String>::new());
}

#[test]
fn test_script_r_directory_predecessor_attach_reaches_sibling() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr"]);

    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/R/a.R"),
        FileRevision::zero(),
        Some("library(dplyr)\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("ws/R/b.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, b.imports(&db)), vec![
        "File(a.R)".to_string(),
        "Package(dplyr)".to_string(),
    ]);
}

#[test]
fn test_script_r_directory_below_uses_full_default_search_path() {
    // Guards against reusing `package_load_layers`'s `base_layer` for the
    // script path: a non-package script sees R's whole startup search path
    // (stats, graphics, ..., base), not just `base`. `package_load_layers`
    // uses `base_layer` because NAMESPACE supplies the rest for a package
    // file; a script has no NAMESPACE.
    let mut db = TestDb::new();
    install_packages(&mut db, &[
        "stats",
        "graphics",
        "grDevices",
        "utils",
        "datasets",
        "methods",
        "base",
    ]);

    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/R/a.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, a.imports(&db)), vec![
        "Package(stats)".to_string(),
        "Package(graphics)".to_string(),
        "Package(grDevices)".to_string(),
        "Package(utils)".to_string(),
        "Package(datasets)".to_string(),
        "Package(methods)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_separate_r_directories_do_not_cross_collate() {
    // Each `R/` directory collates independently, keyed on its parent path,
    // so a monorepo with several `R/` folders doesn't cross-collate.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/one/R/a.R"),
        FileRevision::zero(),
        Some("a_val <- 1\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("ws/two/R/b.R"),
        FileRevision::zero(),
        Some("b_val <- 2\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, a.imports(&db)), Vec::<String>::new());
    assert_eq!(shape(&db, b.imports(&db)), Vec::<String>::new());
}

#[test]
fn test_cross_file_layers_backdates_on_unrelated_script_change() {
    // `collation_siblings` reads every workspace root's `scripts`, so a
    // script added anywhere forces it to re-execute. But the result filtered
    // to `a`/`b`'s own `R/` directory is unchanged, so salsa backdates it and
    // `cross_file_layers` never re-executes.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/R/a.R"),
        FileRevision::zero(),
        Some("a_val <- 1\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("ws/R/b.R"),
        FileRevision::zero(),
        Some("b_val <- 2\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let _ = a.imports(&db);
    assert_eq!(db.executions("cross_file_layers"), 1);

    let elsewhere = File::new(
        &db,
        file_path("ws/other/z.R"),
        FileRevision::zero(),
        Some("z_val <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b, elsewhere]);

    let _ = a.imports(&db);
    assert_eq!(db.executions("cross_file_layers"), 1);
}

#[test]
fn test_sourced_file_inherits_sourcing_files_imports() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let main = File::new(
        &db,
        file_path("w/main.R"),
        FileRevision::zero(),
        Some("x <- 1\nsource(\"helpers.R\")\n".to_string()),
        None,
    );
    let helpers = File::new(
        &db,
        file_path("w/helpers.R"),
        FileRevision::zero(),
        Some("y <- 2\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, helpers.imports(&db)), vec![
        "File(main.R)".to_string()
    ]);
}

#[test]
fn test_inherited_attach_sits_below_sourced_files_own_attaches() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr", "tibble"]);
    let root = workspace_root(&db, "w");
    let main = File::new(
        &db,
        file_path("w/main.R"),
        FileRevision::zero(),
        Some("library(dplyr)\nsource(\"helpers.R\")\n".to_string()),
        None,
    );
    let helpers = File::new(
        &db,
        file_path("w/helpers.R"),
        FileRevision::zero(),
        Some("library(tibble)\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // `helpers.R`'s own attach (tibble) outranks the inherited one (dplyr),
    // same as a collation predecessor's attach would.
    //
    // Tibble appears twice because `source()` forwards a sourced file's own
    // top-level attaches into the sourcing file's index, and that copy is
    // indistinguishable from a `library()` written in `main.R`. The echo is
    // unreachable under `resolve()`'s first-hit-wins search.
    assert_eq!(shape(&db, helpers.imports(&db)), vec![
        "File(main.R)".to_string(),
        "Package(tibble)".to_string(),
        "Package(tibble)".to_string(),
        "Package(dplyr)".to_string(),
    ]);
}

#[test]
fn test_inherited_imports_are_transitive() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let main = File::new(
        &db,
        file_path("w/main.R"),
        FileRevision::zero(),
        Some("source(\"setup.R\")\n".to_string()),
        None,
    );
    let setup = File::new(
        &db,
        file_path("w/setup.R"),
        FileRevision::zero(),
        Some("source(\"helpers.R\")\n".to_string()),
        None,
    );
    let helpers = File::new(
        &db,
        file_path("w/helpers.R"),
        FileRevision::zero(),
        Some("z <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![main, setup, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // `setup.R` outranks `main.R`: it's the more immediate source site.
    assert_eq!(shape(&db, helpers.imports(&db)), vec![
        "File(setup.R)".to_string(),
        "File(main.R)".to_string(),
    ]);
}

#[test]
fn test_multiple_sourcing_files_appear_ordered_by_path() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let a_main = File::new(
        &db,
        file_path("w/a_main.R"),
        FileRevision::zero(),
        Some("source(\"helpers.R\")\n".to_string()),
        None,
    );
    let b_main = File::new(
        &db,
        file_path("w/b_main.R"),
        FileRevision::zero(),
        Some("source(\"helpers.R\")\n".to_string()),
        None,
    );
    let helpers = File::new(
        &db,
        file_path("w/helpers.R"),
        FileRevision::zero(),
        Some("z <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a_main, b_main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, helpers.imports(&db)), vec![
        "File(a_main.R)".to_string(),
        "File(b_main.R)".to_string(),
    ]);
}

#[test]
fn test_inheritance_replaces_collation_instead_of_adding_to_it() {
    // `a.R` and `b.R` would collate together as a non-package `R/` directory
    // (see `test_script_r_directory_siblings_see_each_other`). Once `main.R`
    // explicitly sources `a.R`, `b.R` may never load, so it must drop out of
    // `a.R`'s imports entirely rather than sit alongside the inherited band.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/R/a.R"),
        FileRevision::zero(),
        Some("a_val <- 1\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("ws/R/b.R"),
        FileRevision::zero(),
        Some("b_val <- 2\n".to_string()),
        None,
    );
    let main = File::new(
        &db,
        file_path("ws/main.R"),
        FileRevision::zero(),
        Some("source(\"R/a.R\")\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b, main]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(shape(&db, a.imports(&db)), vec!["File(main.R)".to_string()]);
}

#[test]
fn test_file_nobody_sources_keeps_its_own_cross_file_layers() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr", "base"]);
    let root = workspace_root(&db, "ws");
    let a = File::new(
        &db,
        file_path("ws/R/a.R"),
        FileRevision::zero(),
        Some("library(dplyr)\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("ws/R/b.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // Nobody sources `b.R`, so it keeps exactly the collation view
    // `cross_file_layers` alone would give it.
    assert_eq!(shape(&db, b.imports(&db)), vec![
        "File(a.R)".to_string(),
        "Package(dplyr)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_inherited_default_search_path_is_not_duplicated() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let root = workspace_root(&db, "w");
    let main = File::new(
        &db,
        file_path("w/main.R"),
        FileRevision::zero(),
        Some("source(\"helpers.R\")\n".to_string()),
        None,
    );
    let helpers = File::new(
        &db,
        file_path("w/helpers.R"),
        FileRevision::zero(),
        Some("z <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // `base` comes once from `main.R`'s own `below` band, never duplicated by
    // `helpers.R`'s own (replaced) `cross_file_layers`.
    assert_eq!(shape(&db, helpers.imports(&db)), vec![
        "File(main.R)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_mutual_sourcing_devolves_to_standalone_scripts() {
    // A mutual pair cycles `semantic_index` through `exports`, and the cycling
    // side is rebuilt with `NoopImportsResolver`, whose `resolve_effects`
    // defaults to `None`. So a bare `source()` isn't recognized as effectful on
    // either side and no `SourceSite` survives. The pair never reaches
    // `inherited_layers`' own `cycle_result`, and both files fall back to the
    // standalone-script context.
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let root = workspace_root(&db, "w");
    let a = File::new(
        &db,
        file_path("w/a.R"),
        FileRevision::zero(),
        Some("source(\"b.R\")\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("w/b.R"),
        FileRevision::zero(),
        Some("source(\"a.R\")\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(a.source_sites(&db), &Vec::new());
    assert_eq!(b.source_sites(&db), &Vec::new());
    assert_eq!(a.sourced_by(&db), &Vec::<File>::new());
    assert_eq!(b.sourced_by(&db), &Vec::<File>::new());
    assert_eq!(
        shape(&db, a.imports(&db)),
        vec!["Package(base)".to_string()]
    );
    assert_eq!(
        shape(&db, b.imports(&db)),
        vec!["Package(base)".to_string()]
    );
}

#[test]
fn test_qualified_mutual_sourcing_records_sites_but_no_edges() {
    // `base::source()` stays recognized under `NoopImportsResolver`, whose
    // `resolve_qualified_effects` defaults to `effects::lookup`, so unlike the
    // bare-call pair above both sites survive in the forward projection. They
    // resolve to nothing, though: `resolve_source` reads the target's
    // `exports`, which is the empty cycle fallback. No target means no reverse
    // edge, so inheritance sees a cyclic pair as two standalone scripts.
    //
    // This is what makes `inherited_layers`' own `cycle_result` unreachable
    // rather than load-bearing. Every resolved source site is also a
    // `semantic_index -> exports -> semantic_index` edge in the same direction,
    // so a cycle in source edges always wipes the very targets that would let
    // `inherited_layers` recurse into itself.
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let root = workspace_root(&db, "w");
    let a = File::new(
        &db,
        file_path("w/a.R"),
        FileRevision::zero(),
        Some("base::source(\"b.R\")\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("w/b.R"),
        FileRevision::zero(),
        Some("base::source(\"a.R\")\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(a.source_sites(&db).len(), 1);
    assert_eq!(b.source_sites(&db).len(), 1);
    assert_eq!(a.source_sites(&db)[0].target(), None);
    assert_eq!(b.source_sites(&db)[0].target(), None);

    assert_eq!(a.sourced_by(&db), &Vec::<File>::new());
    assert_eq!(b.sourced_by(&db), &Vec::<File>::new());
    assert_eq!(
        shape(&db, a.imports(&db)),
        vec!["Package(base)".to_string()]
    );
    assert_eq!(
        shape(&db, b.imports(&db)),
        vec!["Package(base)".to_string()]
    );
}

#[test]
fn test_package_r_file_ignores_source_sites() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);

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
    let a = File::new(
        &db,
        file_path("w/pkg/R/a.R"),
        FileRevision::zero(),
        Some("a_val <- 1\n".to_string()),
        Some(pkg),
    );
    let b = File::new(
        &db,
        file_path("w/pkg/R/b.R"),
        FileRevision::zero(),
        Some("b_val <- 2\n".to_string()),
        Some(pkg),
    );
    let dev = File::new(
        &db,
        file_path("w/pkg/data-raw/dev.R"),
        FileRevision::zero(),
        Some("source(\"pkg/R/b.R\")\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![a, b]);
    pkg.set_scripts(&mut db).to(vec![dev]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    // `dev.R` really does source `b.R`, but `Collate:` already says when `b.R`
    // loads, so it keeps its predecessor and NAMESPACE context. Inheriting
    // `dev.R`'s instead would drop `File(a.R)`.
    assert_eq!(b.sourced_by(&db), &vec![dev]);
    assert_eq!(shape(&db, b.imports(&db)), vec![
        "File(a.R)".to_string(),
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
fn test_non_collated_package_file_still_inherits() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);

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
    let helpers = File::new(
        &db,
        file_path("w/pkg/data-raw/helpers.R"),
        FileRevision::zero(),
        Some("h <- 1\n".to_string()),
        Some(pkg),
    );
    let dev = File::new(
        &db,
        file_path("w/pkg/data-raw/dev.R"),
        FileRevision::zero(),
        Some("source(\"pkg/data-raw/helpers.R\")\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db).to(vec![helpers, dev]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    // The gate is about load order, not about carrying a package back-pointer.
    // A `data-raw/` script isn't loaded with the package, so a source site is
    // the only thing that says anything about its environment.
    assert_eq!(shape(&db, helpers.imports(&db)), vec![
        "File(dev.R)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_shadowed_source_call_still_contributes_an_edge() {
    // ```r
    // # a.R                  # b.R              # c.R
    // source <- identity      foo <- 1          foo
    // base::source("b.R")     source("c.R")
    // ```
    //
    // Entered through `a.R`, `b.R`'s `source("c.R")` calls `identity` and `c.R`
    // never loads. But R declares no entry point, and `b.R` run on its own
    // really does source `c.R`. Each file's effects resolve in its standalone
    // context, the one context that assumes nothing about callers, so the edge
    // goes in the map and `c.R` inherits from both files up the chain.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let a = File::new(
        &db,
        file_path("w/a.R"),
        FileRevision::zero(),
        Some("source <- identity\nbase::source(\"b.R\")\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("w/b.R"),
        FileRevision::zero(),
        Some("foo <- 1\nsource(\"c.R\")\n".to_string()),
        None,
    );
    let c = File::new(
        &db,
        file_path("w/c.R"),
        FileRevision::zero(),
        Some("foo\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b, c]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(b.sourced_by(&db), &vec![a]);
    assert_eq!(c.sourced_by(&db), &vec![b]);

    // `b.R` outranks `a.R`, being the more immediate source site. No packages
    // are installed, so nothing follows the two `File` layers.
    assert_eq!(shape(&db, c.imports(&db)), vec![
        "File(b.R)".to_string(),
        "File(a.R)".to_string(),
    ]);
}

#[test]
fn test_body_edit_in_a_sourcing_file_does_not_invalidate_imports() {
    // Inheritance reads the sourcing file's `attach_layers`, hence its `no_eq`
    // `semantic_index`, so `inherited_layers` re-executes on any keystroke in
    // `main.R`. The firewall is one level up: an edit that changes no attach and
    // no source edge returns a value-equal `Vec`, so `helpers.R`'s `imports`
    // backdates and everything downstream of it stays green.
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr"]);
    let root = workspace_root(&db, "w");
    let main = File::new(
        &db,
        file_path("w/main.R"),
        FileRevision::zero(),
        Some("library(dplyr)\nsource(\"helpers.R\")\nf <- function() 1\n".to_string()),
        None,
    );
    let helpers = File::new(
        &db,
        file_path("w/helpers.R"),
        FileRevision::zero(),
        Some("y <- 2\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let before = shape(&db, helpers.imports(&db));
    assert_eq!(before, vec![
        "File(main.R)".to_string(),
        "Package(dplyr)".to_string(),
    ]);
    assert_eq!(db.executions("File::imports"), 1);
    // One per file in the chain: `helpers.R`'s, and `main.R`'s own (empty)
    // inheritance, which `build_inherited_layers` reads for transitivity.
    assert_eq!(db.executions("File::inherited_layers"), 2);

    // Rewrite `f`'s body, leaving the `library()` call and the `source()` call
    // untouched.
    main.set_source_text_override(&mut db).to(Some(
        "library(dplyr)\nsource(\"helpers.R\")\nf <- function() 2 + 2\n".to_string(),
    ));

    assert_eq!(shape(&db, helpers.imports(&db)), before);
    // `helpers.R`'s re-executed and backdated. `main.R`'s didn't: it reads only
    // `sourced_by(main)`, which the edit left alone.
    assert_eq!(db.executions("File::inherited_layers"), 3);
    assert_eq!(db.executions("File::imports"), 1);
}
