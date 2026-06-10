use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use stdext::SortedVec;

use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
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
        .map(|(path, contents)| File::new(db, file_path(path), contents.to_string(), Some(pkg)))
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
fn test_conditional_binding_single_under_last_wins_exports() {
    // Top-level `if (cond) foo <- 1 else foo <- 2` in principle produces two
    // candidate Definitions for `foo`. Full fan-out requires multi-target
    // exports (PR 3, deferred). Under the current single-target model,
    // `file_exports` keeps the last-wins definition, so `Package::resolve`
    // returns exactly one. The stub+onload case across two files still works.
    let mut db = TestDb::new();
    let (pkg, _files) = setup_package(&mut db, "pkg", &["foo"], &[(
        "workspace/pkg/R/a.R",
        "if (cond) foo <- 1 else foo <- 2\n",
    )]);

    let defs = pkg.resolve(&db, name(&db, "foo"), PackageVisibility::Exported);
    assert_eq!(defs.len(), 1);
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
