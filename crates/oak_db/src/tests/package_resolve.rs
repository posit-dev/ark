use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use stdext::SortedVec;

use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Name;
use crate::Package;
use crate::PackageVisibility;

/// Build a `pkg`-named workspace package with files at the given paths and
/// contents. NAMESPACE exports are taken from `exports`. Returns the package
/// and the files in input order.
fn setup_package(
    db: &mut TestDb,
    pkg_name: &str,
    exports: &[&str],
    files: &[(&str, &str)],
) -> (Package, Vec<File>) {
    let root = workspace_root(db, &format!("workspace/{pkg_name}"));
    let namespace = Namespace {
        exports: SortedVec::from_vec(exports.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    let pkg = Package::new(
        db,
        file_path(&format!("workspace/{pkg_name}/DESCRIPTION")),
        pkg_name.to_string(),
        None,
        namespace,
        Vec::new(),
        Vec::new(),
        None,
    );

    let file_entities: Vec<File> = files
        .iter()
        .map(|(path, contents)| {
            File::new(
                db,
                file_path(path),
                FileRevision::zero(),
                Some(contents.to_string()),
                Some(pkg),
            )
        })
        .collect();
    pkg.set_files(db).to(file_entities.clone());
    root.set_packages(db).to(vec![pkg]);
    db.workspace_roots().set_roots(db).to(vec![root]);

    (pkg, file_entities)
}

fn name<'db>(db: &'db TestDb, text: &str) -> Name<'db> {
    Name::new(db, text)
}

#[test]
fn test_exported_visible_via_exported_lookup() {
    let mut db = TestDb::new();
    let (pkg, files) = setup_package(&mut db, "pkg", &["foo"], &[(
        "workspace/pkg/R/a.R",
        "foo <- function() 1\n",
    )]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Exported);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), files[0]);
    assert_eq!(defs[0].name(&db).text(&db).as_str(), "foo");
}

#[test]
fn test_exported_lookup_filters_unexported_name() {
    // `foo` is bound in the package but not in NAMESPACE.exports. The
    // `Exported` lookup must not surface it.
    let mut db = TestDb::new();
    let (pkg, _files) = setup_package(&mut db, "pkg", &[], &[(
        "workspace/pkg/R/a.R",
        "foo <- function() 1\n",
    )]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Exported);
    assert!(defs.is_empty());
}

#[test]
fn test_internal_lookup_finds_unexported_name() {
    // `pkg:::foo` sees the binding even though it's not in NAMESPACE.exports.
    let mut db = TestDb::new();
    let (pkg, files) = setup_package(&mut db, "pkg", &[], &[(
        "workspace/pkg/R/a.R",
        "foo <- function() 1\n",
    )]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Internal);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), files[0]);
}

#[test]
fn test_internal_lookup_also_finds_exported_name() {
    // `:::` is a superset: exported names are also reachable via Internal.
    let mut db = TestDb::new();
    let (pkg, files) = setup_package(&mut db, "pkg", &["foo"], &[(
        "workspace/pkg/R/a.R",
        "foo <- function() 1\n",
    )]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Internal);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), files[0]);
}

#[test]
fn test_unknown_name_returns_empty() {
    let mut db = TestDb::new();
    let (pkg, _files) = setup_package(&mut db, "pkg", &["foo"], &[(
        "workspace/pkg/R/a.R",
        "foo <- function() 1\n",
    )]);

    assert!(pkg
        .resolve(&db, name(&db, "nope"), PackageVisibility::Exported)
        .is_empty());
    assert!(pkg
        .resolve(&db, name(&db, "nope"), PackageVisibility::Internal)
        .is_empty());
}

#[test]
fn test_resolves_binding_in_correct_file_for_multi_file_package() {
    // `foo` is in `a.R`, `bar` is in `b.R`. Each `pkg::name` lookup
    // lands on the right file.
    let mut db = TestDb::new();
    let (pkg, files) = setup_package(&mut db, "pkg", &["foo", "bar"], &[
        ("workspace/pkg/R/a.R", "foo <- function() 1\n"),
        ("workspace/pkg/R/b.R", "bar <- function() 2\n"),
    ]);

    let foo_defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Exported);
    assert_eq!(foo_defs.len(), 1);
    assert_eq!(foo_defs[0].file(&db), files[0]);

    let bar_defs = pkg.resolve(&db, name(&db, "bar"), PackageVisibility::Exported);
    assert_eq!(bar_defs.len(), 1);
    assert_eq!(bar_defs[0].file(&db), files[1]);
}

#[test]
fn test_conditional_binding_fans_out() {
    // Top-level `if (cond) foo <- 1 else foo <- 2` produces two candidate
    // Definitions for `foo`, one per branch. Multi-target exports surface both,
    // so `Package::resolve` returns both candidates from the single file.
    let mut db = TestDb::new();
    let (pkg, _files) = setup_package(&mut db, "pkg", &["foo"], &[(
        "workspace/pkg/R/a.R",
        "if (cond) foo <- 1 else foo <- 2\n",
    )]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Exported);
    assert_eq!(defs.len(), 2);
}

#[test]
fn test_stub_and_onload_override_both_returned() {
    // The R-package idiom: a top-level stub plus a runtime override
    // installed from `.onLoad` via `<<-`. Both bindings live in the
    // package namespace, so `Package::resolve` returns both.
    let mut db = TestDb::new();
    let (pkg, files) = setup_package(&mut db, "pkg", &["foo"], &[
        ("workspace/pkg/R/foo.R", "foo <- function() stop('stub')\n"),
        (
            "workspace/pkg/R/zzz.R",
            ".onLoad <- function(libname, pkgname) {\n  foo <<- function() 'real'\n}\n",
        ),
    ]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Exported);
    assert_eq!(defs.len(), 2);
    let target_files: Vec<File> = defs.iter().map(|d| d.file(&db)).collect();
    assert!(target_files.contains(&files[0]));
    assert!(target_files.contains(&files[1]));
}

#[test]
fn test_reexport_via_import_from_resolves_to_source() {
    // dplyr re-exports tibble's `tibble`: NAMESPACE carries `export(tibble)`
    // plus `importFrom(tibble, tibble)`, and the only R source is a bare
    // `tibble::tibble` expression (not an assignment). The binding lives in
    // tibble, so `dplyr::tibble` must follow the import and resolve there.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "workspace");

    let tibble_ns = Namespace {
        exports: SortedVec::from_vec(vec!["tibble".to_string()]),
        ..Default::default()
    };
    let tibble = Package::new(
        &db,
        file_path("workspace/tibble/DESCRIPTION"),
        "tibble".to_string(),
        None,
        tibble_ns,
        Vec::new(),
        Vec::new(),
        None,
    );
    let tibble_file = File::new(
        &db,
        file_path("workspace/tibble/R/tibble.R"),
        "tibble <- function() 1\n".to_string(),
        Some(tibble),
    );
    tibble.set_files(&mut db).to(vec![tibble_file]);

    let dplyr_ns = Namespace {
        exports: SortedVec::from_vec(vec!["tibble".to_string()]),
        imports: vec![Import {
            name: "tibble".to_string(),
            package: "tibble".to_string(),
        }],
        ..Default::default()
    };
    let dplyr = Package::new(
        &db,
        file_path("workspace/dplyr/DESCRIPTION"),
        "dplyr".to_string(),
        None,
        dplyr_ns,
        Vec::new(),
        Vec::new(),
        None,
    );
    let dplyr_file = File::new(
        &db,
        file_path("workspace/dplyr/R/reexport.R"),
        "tibble::tibble\n".to_string(),
        Some(dplyr),
    );
    dplyr.set_files(&mut db).to(vec![dplyr_file]);

    root.set_packages(&mut db).to(vec![tibble, dplyr]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let defs = dplyr.resolve(&db, name(&db, "tibble"), PackageVisibility::Exported);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), tibble_file);
    assert_eq!(defs[0].name(&db).text(&db).as_str(), "tibble");
}

#[test]
fn test_same_name_defined_in_multiple_files_returns_each() {
    // Two files in the package both define `foo`. `Package::resolve`
    // surfaces both candidates. (R's runtime would have one wins via
    // collation order, but goto-def should offer all candidates.)
    let mut db = TestDb::new();
    let (pkg, files) = setup_package(&mut db, "pkg", &["foo"], &[
        ("workspace/pkg/R/a.R", "foo <- function() 1\n"),
        ("workspace/pkg/R/b.R", "foo <- function() 2\n"),
    ]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Exported);
    assert_eq!(defs.len(), 2);
    let target_files: Vec<File> = defs.iter().map(|d| d.file(&db)).collect();
    assert!(target_files.contains(&files[0]));
    assert!(target_files.contains(&files[1]));
}
