//! Rename at the ide layer.

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_ide::prepare_rename;
use oak_ide::rename;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use url::Url;

use crate::support::install_library_package;
use crate::support::install_workspace_package;
use crate::support::offset;
use crate::support::pairs;
use crate::support::range;
use crate::support::ranges;
use crate::support::upsert;

// --- prepare_rename ---

#[test]
fn test_prepare_rename_on_def() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "foo <- 1\nfoo\n");

    let result = prepare_rename(&db, file, offset(0)).unwrap().unwrap();
    assert_eq!(result, (range(0, 3), "foo".to_string()));
}

#[test]
fn test_prepare_rename_on_use() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "foo <- 1\nfoo\n");

    let result = prepare_rename(&db, file, offset(9)).unwrap().unwrap();
    assert_eq!(result, (range(9, 12), "foo".to_string()));
}

#[test]
fn test_prepare_rename_namespace_access_returns_none() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "dplyr::mutate\n");

    assert!(prepare_rename(&db, file, offset(7)).unwrap().is_none());
}

#[test]
fn test_prepare_rename_non_identifier_returns_none() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\n");

    assert!(prepare_rename(&db, file, offset(3)).unwrap().is_none());
}

#[test]
fn test_prepare_rename_library_package_symbol_errors() {
    let mut db = OakDatabase::new();
    let lib_file = build_library_package_file(&mut db, "foo <- function() {}\n");

    let err = prepare_rename(&db, lib_file, offset(0)).unwrap_err();
    assert!(err.to_string().contains("installed package"));
}

// --- rename: basic ---

#[test]
fn test_rename_def_and_use() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "foo <- 1\nfoo + foo\n");

    let targets = rename(&db, file, offset(0)).unwrap();
    assert_eq!(ranges(&targets), vec![
        range(0, 3),
        range(9, 12),
        range(15, 18)
    ]);
}

#[test]
fn test_rename_excludes_shadowed_outer() {
    let source = "x <- 1\nf <- function() {\n  x <- 2\n  x\n}\n";
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", source);

    let inner_def = source.find("x <- 2").unwrap() as u32;
    let inner_use = source.rfind('x').unwrap() as u32;
    let targets = rename(&db, file, offset(inner_def)).unwrap();
    assert_eq!(ranges(&targets), vec![
        range(inner_def, inner_def + 1),
        range(inner_use, inner_use + 1),
    ]);
}

// --- rename: validation ---

#[test]
fn test_rename_non_renamable_errors() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "dplyr::mutate\n");

    let err = rename(&db, file, offset(7)).unwrap_err();
    assert!(err.to_string().contains("Can't rename identifier"));
}

#[test]
fn test_rename_unbound_use_errors() {
    // Free variable with no binding anywhere in the db.
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "foo\n");

    let err = rename(&db, file, offset(0)).unwrap_err();
    assert!(err.to_string().contains("no binding"));
}

// --- rename: string definitions ---

#[test]
fn test_rename_string_def_spans_quoted_range() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "\"foo\" <- 1\nfoo\n");

    let targets = rename(&db, file, offset(11)).unwrap();
    assert_eq!(ranges(&targets), vec![range(0, 5), range(11, 14)]);
}

// --- rename: cross-file workspace scripts ---

#[test]
fn test_rename_cross_file_workspace_scripts() {
    // Two top-level scripts in a workspace root, linked by `source()`. The
    // candidate scan finds both and the edit spans both files.
    let mut db = OakDatabase::new();
    let helpers = upsert(&mut db, "helpers.R", "helper <- function() 1\n");
    let script = upsert(&mut db, "script.R", "source(\"helpers.R\")\nhelper\n");
    place_in_workspace_scripts(&mut db, vec![helpers, script]);

    let use_start = "source(\"helpers.R\")\n".len() as u32;
    let targets = rename(&db, script, offset(use_start)).unwrap();
    // script.R (cursor's file) first, then helpers.R.
    assert_eq!(pairs(&targets), vec![
        (script, range(use_start, use_start + 6)),
        (helpers, range(0, 6)),
    ]);
}

// --- rename: cross-file workspace package ---

#[test]
fn test_rename_cross_file_workspace_package() {
    // `shared` is defined in a.R and used by a function body in b.R, a sibling
    // file in the same workspace package. Package collation makes b.R's use
    // resolve to a.R's def, so the rename spans both package files.
    let mut db = OakDatabase::new();
    let files = build_workspace_package(&mut db, &[
        ("a.R", "shared <- 1\n"),
        ("b.R", "use_shared <- function() shared\n"),
    ]);
    let (a, b) = (files[0], files[1]);

    // Cursor on the def `shared` in a.R at offset 0.
    let targets = rename(&db, a, offset(0)).unwrap();
    let use_start = "use_shared <- function() ".len() as u32;
    assert_eq!(pairs(&targets), vec![
        (a, range(0, 6)),
        (b, range(use_start, use_start + 6)),
    ]);
}

// --- rename: installed packages ---

#[test]
fn test_rename_refuses_library_package_symbol() {
    // Symbol defined in an installed-package file. Even with the file open and
    // its sources available, editing it wouldn't change what's installed, so
    // rename refuses.
    let mut db = OakDatabase::new();
    let lib_file = build_library_package_file(&mut db, "foo <- function() {}\n");

    // Cursor on the def `foo` at offset 0.
    let err = rename(&db, lib_file, offset(0)).unwrap_err();
    assert!(err.to_string().contains("installed package"));
}

#[test]
fn test_rename_refuses_package_export_used_via_library() {
    // `library(mypkg)` then a use of its exported `foo`. The use now resolves
    // through the package layer to the installed-package binding, so rename
    // refuses with the installed-package guard. Before package-layer resolution
    // this use was unbound and errored with "no binding" instead.
    let mut db = OakDatabase::new();
    let _pkg_file =
        install_library_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");
    let script = upsert(&mut db, "script.R", "library(mypkg)\nfoo\n");

    let use_start = "library(mypkg)\n".len() as u32;
    let err = rename(&db, script, offset(use_start)).unwrap_err();
    assert!(err.to_string().contains("installed package"));
}

#[test]
fn test_rename_succeeds_for_workspace_package_export_via_library() {
    // `library(mypkg)` where `mypkg` is a *workspace* package. Unlike an
    // installed package, workspace files are editable, so rename must succeed
    // and rewrite both the script use and the definition in the package file.
    let mut db = OakDatabase::new();
    let pkg_file =
        install_workspace_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");
    let script = upsert(&mut db, "script.R", "library(mypkg)\nfoo\n");

    let use_start = "library(mypkg)\n".len() as u32;
    let result = rename(&db, script, offset(use_start), "bar").unwrap();
    assert_eq!(pairs(&result.ranges), vec![
        (script, range(use_start, use_start + 3)),
        (pkg_file, range(0, 3)),
    ]);
}

// --- helpers for root / package wiring ---

fn place_in_workspace_scripts(db: &mut OakDatabase, files: Vec<File>) {
    // Root path must be an ancestor of the files' URLs (see `file_url`), as a
    // real scan guarantees: `File::root` resolves an unpackaged file to the
    // root whose scan reached it, and `source()` anchoring reads that root's
    // path.
    let raw = if cfg!(windows) {
        "file:///C:/project/R/"
    } else {
        "file:///project/R/"
    };
    let url = FilePath::from_url(&Url::parse(raw).unwrap());
    let root = Root::new(db, url, RootKind::Workspace, files, vec![]);
    db.workspace_roots().set_roots(db).to(vec![root]);
}

/// Build a workspace package holding `files` (name, contents), each with the
/// package back-pointer set, and register it under a workspace root. Returns
/// the created `File`s in order.
fn build_workspace_package(db: &mut OakDatabase, files: &[(&str, &str)]) -> Vec<File> {
    let pkg = empty_package(db, "file:///project/pkg/DESCRIPTION", None);
    let created: Vec<File> = files
        .iter()
        .map(|(name, contents)| {
            let url =
                FilePath::from_url(&Url::parse(&format!("file:///project/pkg/R/{name}")).unwrap());
            File::new(db, url, contents.to_string(), Some(pkg))
        })
        .collect();
    pkg.set_files(db).to(created.clone());

    let root_url = FilePath::from_url(&Url::parse("file:///project/pkg/").unwrap());
    let root = Root::new(db, root_url, RootKind::Workspace, vec![], vec![pkg]);
    db.workspace_roots().set_roots(db).to(vec![root]);
    created
}

/// Build a single installed-package file under a library root.
fn build_library_package_file(db: &mut OakDatabase, contents: &str) -> File {
    let pkg = empty_package(db, "file:///lib/pkg/DESCRIPTION", Some("1.0".to_string()));
    let url = FilePath::from_url(&Url::parse("file:///lib/pkg/R/foo.R").unwrap());
    let file = File::new(db, url, contents.to_string(), Some(pkg));
    pkg.set_files(db).to(vec![file]);

    let root_url = FilePath::from_url(&Url::parse("file:///lib/").unwrap());
    let root = Root::new(db, root_url, RootKind::Library, vec![], vec![pkg]);
    db.library_roots().set_roots(db).to(vec![root]);
    file
}

fn empty_package(db: &OakDatabase, description_url: &str, version: Option<String>) -> Package {
    Package::new(
        db,
        FilePath::from_url(&Url::parse(description_url).unwrap()),
        "pkg".to_string(),
        version,
        Namespace::default(),
        vec![],
        vec![],
        None,
    )
}
