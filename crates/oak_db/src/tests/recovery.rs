//! Verifies the recovery handlers Salsa selects for known query cycles.
//!
//! Tests clear [`recovery`] immediately before querying a cycle because Salsa
//! memoizes the cycle result and a warm database might not invoke a handler
//! again.

use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use stdext::SortedVec;

use crate::file_imports::CollationView;
use crate::recovery;
use crate::tests::file_imports::install_packages;
use crate::tests::test_db::file_path;
use crate::tests::test_db::make_package;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Name;
use crate::NamespaceVisibility;

/// Sorted so the assertion doesn't pin salsa's traversal order. Deduplicated
/// because salsa re-runs cycle participants to fixpoint, making a handler's
/// firing count an iteration artifact rather than behaviour.
fn assert_fired(expected: &[&str]) {
    let mut fired = recovery::fired();
    fired.sort();
    fired.dedup();
    assert_eq!(fired, expected);
}

/// Assert that `inherited_layers()` stayed out of the cycle.
///
/// The `semantic_index()` check keeps this from passing vacuously. A fixture
/// that stopped forming a cycle would record nothing at all, and the absence
/// assertion alone would still hold. [`crate::tests::cycle_results`] guards the
/// same hazard with its `assert_fixture_cycles()`.
fn assert_inherited_layers_did_not_fire() {
    let fired = recovery::fired();
    assert!(fired
        .iter()
        .any(|entry| entry.starts_with("semantic_index(")));
    assert!(!fired
        .iter()
        .any(|entry| entry.starts_with("inherited_layers(")));
}

/// Register loose scripts under a fresh workspace root, returned in the order
/// they are declared.
fn scripts(db: &mut TestDb, root_path: &str, sources: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(&*db, root_path);
    let files: Vec<File> = sources
        .iter()
        .map(|(path, text)| {
            File::new(
                &*db,
                file_path(path),
                FileRevision::zero(),
                Some(text.to_string()),
                None,
            )
        })
        .collect();
    root.set_scripts(db).to(files.clone());
    let roots = db.workspace_roots();
    roots.set_roots(db).to(vec![root]);
    files
}

// == Matrix rows ==

#[test]
fn test_semantic_index_and_exports_fire_on_mutual_source_cycle() {
    // Bare mutual `source()` pair, the shape of
    // `test_mutual_sourcing_devolves_to_standalone_scripts`
    // (`tests/file_imports.rs`) and `test_cyclic_source_returns_empty_exports_without_panicking`
    // (`tests/file_exports.rs`).
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let files = scripts(&mut db, "w", &[
        ("w/a.R", "source(\"b.R\")\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);

    recovery::reset();
    let _ = files[0].semantic_index(&db);

    // `source_resolution()` reads both `exports()` and `attached_packages()`
    // for the target, so `attached_packages` rides along even though the
    // fixture has no `library()` calls.
    assert_fired(&[
        "attached_packages(w/a.R)",
        "attached_packages(w/b.R)",
        "exports(w/a.R)",
        "exports(w/b.R)",
        "semantic_index(w/a.R)",
        "semantic_index(w/b.R)",
    ]);
}

#[test]
fn test_attached_packages_fires_and_anywhere_does_not() {
    // Package analog of the #15631 ring, adapted from the loose-script
    // `R/` collation fallback removed in `test_r_directory_collation_with_a_source_call_does_not_panic`
    // (`tests/workspace.rs`); package collation reaches the same cycle.
    // Touching `c.R` first is what makes salsa re-enter `attached_packages`
    // rather than `semantic_index`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["pkga", "pkgb", "pkgc"]);
    let (pkg, files) = make_package(&mut db, "proj", Namespace::default(), &[
        ("ws/proj/R/a.R", "library(pkga)\nsource(\"R/b.R\")\n"),
        ("ws/proj/R/b.R", "library(pkgb)\n"),
        ("ws/proj/R/c.R", "library(pkgc)\n"),
    ]);
    let root = workspace_root(&db, "ws/proj");
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    recovery::reset();
    let _ = files[2].used_packages(&db);

    assert_fired(&[
        "attached_packages(ws/proj/R/a.R)",
        "attached_packages(ws/proj/R/b.R)",
        "cross_file_layers(ws/proj/R/b.R, Eager)",
        "exports(ws/proj/R/a.R)",
        "exports(ws/proj/R/b.R)",
        "semantic_index(ws/proj/R/a.R)",
        "semantic_index(ws/proj/R/b.R)",
    ]);
}

#[test]
fn test_cross_file_layers_fires_on_cold_entry() {
    // `test_cold_entry_to_cross_file_layers_recovers` (`tests/file_imports.rs`).
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "pkga"]);
    let (pkg, files) = make_package(&mut db, "mypkg", Namespace::default(), &[
        ("ws/mypkg/R/a.R", "library(pkga)\nsource(\"R/b.R\")\n"),
        ("ws/mypkg/R/b.R", "library(pkga)\n"),
    ]);
    let root = workspace_root(&db, "ws/mypkg");
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    recovery::reset();
    let _ = files[1].cross_file_layers(&db, CollationView::Eager);

    // `cross_file_layers(b, Eager)` is the repeated key; `attached_packages`,
    // `exports` and `semantic_index` ride along on both sides of the ring.
    assert_fired(&[
        "attached_packages(ws/mypkg/R/a.R)",
        "attached_packages(ws/mypkg/R/b.R)",
        "cross_file_layers(ws/mypkg/R/b.R, Eager)",
        "exports(ws/mypkg/R/a.R)",
        "exports(ws/mypkg/R/b.R)",
        "semantic_index(ws/mypkg/R/a.R)",
        "semantic_index(ws/mypkg/R/b.R)",
    ]);
}

#[test]
fn test_package_resolve_fires_on_mutual_reexport() {
    // `test_mutual_reexport_does_not_cycle` (`tests/package_resolve.rs`).
    let mut db = TestDb::new();
    let root = workspace_root(&db, "workspace");

    let namespace_a = Namespace {
        exports: SortedVec::from_vec(vec!["foo".to_string()]),
        imports: vec![Import {
            name: "foo".to_string(),
            package: "b".to_string(),
        }],
        ..Default::default()
    };
    let (a, _a_files) = make_package(&mut db, "a", namespace_a, &[(
        "workspace/a/R/a.R",
        "b::foo\n",
    )]);

    let namespace_b = Namespace {
        exports: SortedVec::from_vec(vec!["foo".to_string()]),
        imports: vec![Import {
            name: "foo".to_string(),
            package: "a".to_string(),
        }],
        ..Default::default()
    };
    let (b, _b_files) = make_package(&mut db, "b", namespace_b, &[(
        "workspace/b/R/b.R",
        "a::foo\n",
    )]);

    root.set_packages(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    recovery::reset();
    let _ = a.resolve(&db, Name::new(&db, "foo"), NamespaceVisibility::Exported);

    assert_fired(&[
        "Package::resolve(a, foo, Exported)",
        "Package::resolve(b, foo, Exported)",
    ]);
}

// == `inherited_layers()` recovery does not run for source cycles ==
//
// Each test enters `inherited_layers()` cold. Warming `semantic_index()` first
// would collapse the cycle before the query runs, so the assertion would hold
// even if this query could participate.
//
// One fixture per distinct route into the query: a plain `source()` ring,
// path-based `sourceDir()` expansion, and a ring closed by an edit against a
// warm cache. Ring size and call qualification do not change the route.

#[test]
fn test_inherited_layers_never_fires_on_bare_mutual_pair() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let files = scripts(&mut db, "w", &[
        ("w/a.R", "source(\"b.R\")\n"),
        ("w/b.R", "source(\"a.R\")\n"),
    ]);

    recovery::reset();
    for file in &files {
        let _ = file.inherited_layers(&db, CollationView::Eager);
    }

    assert_inherited_layers_did_not_fire();
}

#[test]
fn test_inherited_layers_never_fires_on_source_dir_mutual_pair() {
    // `sourceDir()` expansion (`source_dir_scripts`) is path-based, so unlike
    // `source()` the sourced-file list doesn't route through `exports()`.
    let mut db = TestDb::new();
    let files = scripts(&mut db, "ws", &[
        ("ws/dirA/a.R", "sourceDir(\"dirB\")\n"),
        ("ws/dirB/b.R", "sourceDir(\"dirA\")\n"),
    ]);

    recovery::reset();
    for file in &files {
        let _ = file.inherited_layers(&db, CollationView::Eager);
    }

    assert_inherited_layers_did_not_fire();
}

#[test]
fn test_inherited_layers_never_fires_on_ring_closed_by_edit() {
    // Acyclic pair, queried so `a`'s index memoizes from a prior revision,
    // then `b` is edited to close the ring and both are queried again.
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let files = scripts(&mut db, "w", &[
        ("w/a.R", "source(\"b.R\")\n"),
        ("w/b.R", "b_val <- 1\n"),
    ]);
    let (a, b) = (files[0], files[1]);

    // Warm `a`'s index against the acyclic pair.
    let _ = a.semantic_index(&db);

    recovery::reset();
    b.set_source_text_override(&mut db)
        .to(Some("source(\"a.R\")\n".to_string()));
    let _ = a.inherited_layers(&db, CollationView::Eager);
    let _ = b.inherited_layers(&db, CollationView::Eager);

    assert_inherited_layers_did_not_fire();
}
