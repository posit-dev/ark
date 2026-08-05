use std::fs;

use aether_path::FilePath;
use salsa::Setter;

use crate::all_package_dependencies;
use crate::tests::test_db::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Package;

/// Register `names` as installed packages (empty sources) under a single
/// library root.
fn register_library(db: &mut TestDb, names: &[&str]) {
    let lib = library_root(&*db, "libs");
    let packages: Vec<Package> = names
        .iter()
        .map(|name| {
            Package::new(
                &*db,
                file_path(&format!("libs/{name}/DESCRIPTION")),
                name.to_string(),
                FileRevision::zero(),
                FileRevision::zero(),
                None,
                None,
                vec![],
                Vec::new(),
            )
        })
        .collect();
    lib.set_packages(db).to(packages);
    db.library_roots().set_roots(db).to(vec![lib]);
}

/// Add a workspace root at `proj` holding one script with `contents` injected
/// as the editor override (so the semantic index sees it without disk).
fn workspace_with_script(db: &mut TestDb, contents: &str) {
    let root = workspace_root(&*db, "proj");
    let file = File::new(
        &*db,
        file_path("proj/script.R"),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    );
    root.set_scripts(db).to(vec![file]);
    db.workspace_roots().set_roots(db).to(vec![root]);
}

/// Create a `proj` workspace root with editor overrides for `scripts`.
/// Paths are relative to `proj`.
fn workspace_with_scripts(db: &mut TestDb, scripts: &[(&str, &str)]) {
    let root = workspace_root(&*db, "proj");
    let files: Vec<File> = scripts
        .iter()
        .map(|(path, contents)| {
            File::new(
                &*db,
                file_path(&format!("proj/{path}")),
                FileRevision::zero(),
                Some(contents.to_string()),
                None,
            )
        })
        .collect();
    root.set_scripts(db).to(files);
    db.workspace_roots().set_roots(db).to(vec![root]);
}

fn all_package_dependencies_names(db: &TestDb) -> Vec<String> {
    all_package_dependencies(db)
        .iter()
        .map(|&pkg| pkg.name(db).clone())
        .collect()
}

#[test]
fn test_collects_library_and_namespace_accesses() {
    let mut db = TestDb::new();
    register_library(&mut db, &["foo", "bar", "unused"]);
    workspace_with_script(&mut db, "library(foo)\nbar::thing()\n");

    // `foo` via `library()`, `bar` via `::`; `unused` is installed but never
    // referenced. Sorted by name.
    assert_eq!(all_package_dependencies_names(&db), vec!["bar", "foo"]);
}

#[test]
fn test_collects_internal_namespace_access() {
    let mut db = TestDb::new();
    register_library(&mut db, &["rlang"]);
    workspace_with_script(&mut db, "rlang:::abort()\n");

    assert_eq!(all_package_dependencies_names(&db), vec!["rlang"]);
}

#[test]
fn test_collects_imports_and_depends_from_workspace_package() {
    let dir = tempfile::tempdir().unwrap();
    let description = dir.path().join("DESCRIPTION");
    fs::write(
        &description,
        "Package: mypkg\nVersion: 1.0.0\nImports: rlang\nDepends: cli\n",
    )
    .unwrap();

    let mut db = TestDb::new();
    register_library(&mut db, &["rlang", "cli", "unused"]);

    let root = workspace_root(&db, "proj");
    let pkg = Package::new(
        &db,
        FilePath::from_path_buf(description).unwrap(),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        vec![],
        Vec::new(),
    );
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // `cli` via `Depends`, `rlang` via `Imports`. The workspace package
    // `mypkg` itself is not included (it resolves to a workspace root).
    assert_eq!(all_package_dependencies_names(&db), vec!["cli", "rlang"]);
}

#[test]
fn test_uninstalled_name_is_dropped() {
    let mut db = TestDb::new();
    register_library(&mut db, &["foo"]);
    workspace_with_script(&mut db, "library(ghost)\nfoo::thing()\n");

    // `ghost` has no installed entity to populate, so only `foo` survives.
    assert_eq!(all_package_dependencies_names(&db), vec!["foo"]);
}

#[test]
fn test_workspace_package_self_reference_is_dropped() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "proj");
    let pkg = Package::new(
        &db,
        file_path("proj/DESCRIPTION"),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        vec![],
        Vec::new(),
    );
    let file = File::new(
        &db,
        file_path("proj/R/a.R"),
        FileRevision::zero(),
        Some("mypkg::helper()\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // `mypkg::helper` resolves to the workspace package, which already has its
    // source, so it's dropped.
    assert!(all_package_dependencies_names(&db).is_empty());
}

#[test]
fn test_empty_workspace_uses_nothing() {
    let db = TestDb::new();
    assert!(all_package_dependencies_names(&db).is_empty());
}

#[test]
fn test_default_search_path_packages_are_always_available_to_workspace_scripts() {
    let mut db = TestDb::new();
    register_library(&mut db, &[
        "base",
        "datasets",
        "grDevices",
        "graphics",
        "methods",
        "stats",
        "utils",
        "unused",
    ]);
    // A script that attaches nothing and references no package.
    workspace_with_script(&mut db, "1 + 1\n");

    // Every installed default search path package is implicitly available, even
    // though the script never references them. `unused` isn't on the search
    // path and is never referenced, so it drops out.
    assert_eq!(all_package_dependencies_names(&db), vec![
        "base",
        "datasets",
        "grDevices",
        "graphics",
        "methods",
        "stats",
        "utils"
    ]);
}

#[test]
fn test_default_search_path_packages_are_always_available_to_workspace_packages() {
    let mut db = TestDb::new();
    register_library(&mut db, &["rlang", "base", "stats"]);

    // Build a workspace package that only imports rlang
    let dir = tempfile::tempdir().unwrap();
    let description = dir.path().join("DESCRIPTION");
    fs::write(
        &description,
        "Package: mypkg\nVersion: 1.0.0\nImports: rlang\n",
    )
    .unwrap();

    let pkg = Package::new(
        &db,
        FilePath::from_path_buf(description).unwrap(),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        vec![],
        Vec::new(),
    );

    let root = workspace_root(&db, "proj");
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // `mypkg` only declares `rlang` in `Imports` and nothing in `Depends`, but
    // the default search path packages `base` and `stats` are always available
    // even though the `DESCRIPTION` never mentions them.
    assert_eq!(all_package_dependencies_names(&db), vec![
        "base", "rlang", "stats"
    ]);
}

#[test]
fn test_shiny_app_depends_on_shiny_without_a_library_call() {
    // `loadSupport()` attaches `shiny`, so Shiny apps do not call `library(shiny)`.
    // Include its sources so definitions imported from `shiny` resolve.
    let mut db = TestDb::new();
    register_library(&mut db, &["shiny"]);
    workspace_with_scripts(&mut db, &[("app.R", "shinyApp(ui, server)\n")]);

    assert_eq!(all_package_dependencies_names(&db), vec!["shiny"]);
}

#[test]
fn test_shiny_support_files_depend_on_shiny() {
    // `loadSupport()` also sources `global.R` and `R/` files with `shiny` attached.
    let mut db = TestDb::new();
    register_library(&mut db, &["shiny"]);
    workspace_with_scripts(&mut db, &[
        ("app.R", "shinyApp(ui, server)\n"),
        ("global.R", "app_title <- \"Playground\"\n"),
        ("R/greet.R", "greet <- function(who) who\n"),
    ]);

    assert_eq!(all_package_dependencies_names(&db), vec!["shiny"]);
}

#[test]
fn test_non_shiny_workspace_does_not_depend_on_shiny() {
    // Only Shiny's app marker call enables its implicit attach.
    let mut db = TestDb::new();
    register_library(&mut db, &["shiny"]);
    workspace_with_scripts(&mut db, &[("app.R", "1 + 1\n")]);

    assert!(all_package_dependencies_names(&db).is_empty());
}

#[test]
fn test_uninstalled_implicit_attach_is_dropped() {
    // Implicit attachments require an installed package, like explicitly named dependencies.
    let mut db = TestDb::new();
    register_library(&mut db, &["foo"]);
    workspace_with_scripts(&mut db, &[("app.R", "shinyApp(ui, server)\n")]);

    assert!(all_package_dependencies_names(&db).is_empty());
}

#[test]
fn test_testthat_file_depends_on_testthat() {
    // testthat attaches itself while it runs test files, so unqualified expectations
    // need its sources.
    let mut db = TestDb::new();
    register_library(&mut db, &["testthat"]);

    let root = workspace_root(&db, "proj");
    let pkg = Package::new(
        &db,
        file_path("proj/DESCRIPTION"),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        vec![],
        Vec::new(),
    );
    let test = File::new(
        &db,
        file_path("proj/tests/testthat/test-greet.R"),
        FileRevision::zero(),
        Some("expect_equal(1, 1)\n".to_string()),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![test]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(all_package_dependencies_names(&db), vec!["testthat"]);
}
