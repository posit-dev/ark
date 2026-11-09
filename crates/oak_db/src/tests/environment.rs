//! File-to-environment lookup through `File::environment()`, and its agreement
//! with the `source()` anchor that shares `workspace_anchor()`.

use std::collections::HashSet;

use aether_path::FilePath;
use camino::Utf8Path;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::test_path::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::make_package;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::Environment;
use crate::EnvironmentStatus;
use crate::File;
use crate::FileRevision;
use crate::Notebook;
use crate::Root;

fn make_file(db: &TestDb, name: &str) -> File {
    File::new(db, file_path(name), FileRevision::zero(), None, None)
}

fn unresolved(db: &TestDb, dir: &str) -> Environment {
    Environment::new(
        db,
        Some(file_path(dir)),
        EnvironmentStatus::Unresolved,
        None,
    )
}

fn set_environments(db: &mut TestDb, environments: Vec<Environment>) {
    db.workspace_environments()
        .set_environments(db)
        .to(environments);
}

/// Workspace root `proj` with scripts `files`, environment directories `dirs`,
/// and a live environment for the root and for each directory.
fn workspace(db: &mut TestDb, files: &[&str], dirs: &[&str]) -> (Root, Vec<Environment>) {
    let root = workspace_root(db, "proj");
    let scripts: Vec<File> = files.iter().map(|name| make_file(db, name)).collect();
    root.set_scripts(db).to(scripts);
    root.set_environment_dirs(db)
        .to(dirs.iter().map(|dir| file_path(dir)).collect());
    db.workspace_roots().set_roots(db).to(vec![root]);

    let mut environments = vec![unresolved(db, "proj")];
    for dir in dirs.iter().filter(|dir| **dir != "proj") {
        environments.push(unresolved(db, dir));
    }
    set_environments(db, environments.clone());
    (root, environments)
}

fn script(db: &TestDb, root: Root, name: &str) -> File {
    let path = file_path(name);
    match root.scripts(db).iter().find(|file| file.path(db) == &path) {
        Some(&file) => file,
        None => panic!("No script {name}"),
    }
}

/// The environment's directory is the `source()` anchor, and files outside the
/// workspace anchor at their parent.
fn assert_anchor_agrees(db: &TestDb, file: File) {
    let anchor = file.anchor_dir(db);
    match file.environment(db).dir(db) {
        Some(dir) => assert_eq!(anchor.as_deref(), dir.as_path()),
        None => {
            let parent = file.path(db).as_path().and_then(Utf8Path::parent);
            assert_eq!(anchor.as_deref(), parent);
        },
    }
}

#[test]
fn test_root_file_maps_to_root_environment() {
    let mut db = TestDb::new();
    let (root, environments) = workspace(&mut db, &["proj/a.R"], &["proj/sub"]);
    let file = script(&db, root, "proj/a.R");

    assert_eq!(file.environment(&db), environments[0]);
    assert_anchor_agrees(&db, file);
}

#[test]
fn test_nested_file_maps_to_nearest_environment_dir() {
    let mut db = TestDb::new();
    let (root, environments) = workspace(&mut db, &["proj/sub/deep/b.R", "proj/other/c.R"], &[
        "proj/sub",
        "proj/sub/deep",
    ]);

    let nested = script(&db, root, "proj/sub/deep/b.R");
    assert_eq!(nested.environment(&db), environments[2]);
    assert_anchor_agrees(&db, nested);

    let sibling = script(&db, root, "proj/other/c.R");
    assert_eq!(sibling.environment(&db), environments[0]);
    assert_anchor_agrees(&db, sibling);
}

#[test]
fn test_root_that_is_environment_dir_maps_to_its_one_environment() {
    let mut db = TestDb::new();
    let (root, environments) = workspace(&mut db, &["proj/a.R"], &["proj"]);
    assert_eq!(environments.len(), 1);
    let file = script(&db, root, "proj/a.R");

    assert_eq!(file.environment(&db), environments[0]);
    assert_anchor_agrees(&db, file);
}

#[test]
fn test_buffer_opened_before_scan_maps_to_root_environment() {
    // The root exists, empty, before its first scan. Its environment
    // directories arrive with that scan.
    let mut db = TestDb::new();
    let (_, environments) = workspace(&mut db, &[], &[]);
    let file = make_file(&db, "proj/sub/a.R");
    db.orphan_root()
        .set_files(&mut db)
        .to(HashSet::from([file]));

    assert_eq!(file.environment(&db), environments[0]);
    assert_anchor_agrees(&db, file);
}

#[test]
fn test_orphan_outside_roots_maps_to_default() {
    let mut db = TestDb::new();
    workspace(&mut db, &[], &[]);
    let file = make_file(&db, "elsewhere/a.R");
    db.orphan_root()
        .set_files(&mut db)
        .to(HashSet::from([file]));

    assert_eq!(file.environment(&db), db.default_environment());
    assert_anchor_agrees(&db, file);
}

#[test]
fn test_virtual_document_maps_to_default() {
    let mut db = TestDb::new();
    workspace(&mut db, &[], &[]);
    let path = FilePath::parse("untitled:Untitled-1").unwrap();
    let file = File::new(&db, path, FileRevision::zero(), Some(String::new()), None);
    db.orphan_root()
        .set_files(&mut db)
        .to(HashSet::from([file]));

    assert_eq!(file.environment(&db), db.default_environment());
    assert_anchor_agrees(&db, file);
}

/// Open one notebook at `notebook`, replacing any open ones, and return its
/// only cell.
fn notebook_cell(db: &mut TestDb, notebook: &str) -> File {
    let cell_path = FilePath::parse(&format!("vscode-notebook-cell:/{notebook}#W0s")).unwrap();
    let cell = File::new(
        db,
        cell_path,
        FileRevision::zero(),
        Some(String::new()),
        None,
    );
    let notebook = Notebook::new(db, file_path(notebook), vec![cell]);
    db.open_notebooks().set_notebooks(db).to(vec![notebook]);
    cell
}

#[test]
fn test_notebook_cell_maps_to_its_notebook_environment() {
    let mut db = TestDb::new();
    let (_, environments) = workspace(&mut db, &[], &["proj/sub"]);

    let root_cell = notebook_cell(&mut db, "proj/analysis.ipynb");
    assert_eq!(root_cell.environment(&db), environments[0]);

    let nested_cell = notebook_cell(&mut db, "proj/sub/deep/analysis.ipynb");
    assert_eq!(nested_cell.environment(&db), environments[1]);

    // Selecting the notebook's environment does not give the cell a `source()` anchor.
    assert_eq!(nested_cell.anchor_dir(&db), None);
}

#[test]
fn test_notebook_cell_outside_roots_maps_to_default() {
    let mut db = TestDb::new();
    workspace(&mut db, &[], &[]);
    let cell = notebook_cell(&mut db, "elsewhere/analysis.ipynb");

    assert_eq!(cell.environment(&db), db.default_environment());
}

#[test]
fn test_library_file_maps_to_default() {
    // A library inside a workspace root still belongs to the default.
    let mut db = TestDb::new();
    workspace(&mut db, &[], &["proj/lib"]);
    let library = library_root(&db, "proj/lib");
    let (package, files) = make_package(&mut db, "pkg", Namespace::default(), &[(
        "proj/lib/pkg/R/a.R",
        "",
    )]);
    library.set_packages(&mut db).to(vec![package]);
    db.library_roots().set_roots(&mut db).to(vec![library]);
    assert_eq!(files[0].root(&db), Some(library));

    assert_eq!(files[0].environment(&db), db.default_environment());
    assert_anchor_agrees(&db, files[0]);
}

#[test]
fn test_directory_listed_by_two_nested_roots_maps_to_one_environment() {
    // Both roots list `proj/inner/env`. `outer.R` is reached only through
    // the outer root's scan and `inner.R` only through the inner root's.
    let mut db = TestDb::new();
    let outer = workspace_root(&db, "proj");
    let inner = workspace_root(&db, "proj/inner");
    let outer_file = make_file(&db, "proj/inner/env/outer.R");
    let inner_file = make_file(&db, "proj/inner/env/inner.R");
    outer.set_scripts(&mut db).to(vec![outer_file]);
    inner.set_scripts(&mut db).to(vec![inner_file]);
    for root in [outer, inner] {
        root.set_environment_dirs(&mut db)
            .to(vec![file_path("proj/inner/env")]);
    }
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![outer, inner]);
    let env = unresolved(&db, "proj/inner/env");
    let environments = vec![unresolved(&db, "proj"), env, unresolved(&db, "proj/inner")];
    set_environments(&mut db, environments);
    assert_eq!(outer_file.root(&db), Some(outer));
    assert_eq!(inner_file.root(&db), Some(inner));

    assert_eq!(outer_file.environment(&db), env);
    assert_eq!(inner_file.environment(&db), env);
    assert_anchor_agrees(&db, outer_file);
    assert_anchor_agrees(&db, inner_file);
}

#[test]
fn test_environment_dir_without_live_environment_falls_back_to_root() {
    // A scan can list a directory before the live environments are
    // reconciled to it.
    let mut db = TestDb::new();
    let (root, environments) = workspace(&mut db, &["proj/sub/a.R"], &[]);
    root.set_environment_dirs(&mut db)
        .to(vec![file_path("proj/sub")]);
    let file = script(&db, root, "proj/sub/a.R");

    assert_eq!(file.environment(&db), environments[0]);
}

#[test]
fn test_environment_follows_live_environment_changes() {
    let mut db = TestDb::new();
    let (root, environments) = workspace(&mut db, &["proj/sub/a.R"], &["proj/sub"]);
    let file = script(&db, root, "proj/sub/a.R");
    assert_eq!(file.environment(&db), environments[1]);

    set_environments(&mut db, vec![environments[0]]);
    assert_eq!(file.environment(&db), environments[0]);

    set_environments(&mut db, Vec::new());
    assert_eq!(file.environment(&db), db.default_environment());
}

#[test]
fn test_unmanaged_database_maps_files_to_resolved_default() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "proj");
    let file = make_file(&db, "proj/a.R");
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let default = db.default_environment();
    assert_eq!(file.environment(&db), default);
    assert_eq!(default.dir(&db), &None);
    assert_eq!(
        default.status(&db),
        &EnvironmentStatus::Resolved(Vec::new())
    );
    assert_eq!(default.last_error(&db), &None);
}
