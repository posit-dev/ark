use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::RootKind;
use oak_db::SourceDb;
use oak_package_metadata::namespace::Import;
use oak_scan::DbScan;
use salsa::Setter;

/// Write a minimal installed package under `dir`: a `DESCRIPTION` file
/// with `Package: {name}, Version: 1.0.0`, the `Meta/package.rds` marker
/// the library listing requires, plus the `R/` files in `r_files`
/// (basename -> contents).
fn write_package(dir: &Path, name: &str, r_files: &[(&str, &str)]) {
    fs::create_dir_all(dir.join("R")).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 1.0.0\n"),
    )
    .unwrap();
    mark_installed(dir);
    for (basename, contents) in r_files {
        fs::write(dir.join("R").join(basename), contents).unwrap();
    }
}

fn mark_installed(dir: &Path) {
    fs::create_dir_all(dir.join("Meta")).unwrap();
    fs::write(dir.join("Meta").join("package.rds"), "").unwrap();
}

#[test]
fn test_scan_empty_library_path_registers_empty_root() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let roots = db.library_roots().roots(&db).clone();
    assert_eq!(roots.len(), 1);
    let root = roots[0];
    assert_eq!(root.kind(&db), RootKind::Library);
    assert!(root.packages(&db).is_empty());
}

#[test]
fn test_scan_library_discovers_package() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("dplyr"), "dplyr", &[
        ("mutate.R", "mutate <- function(x) x\n"),
        ("select.R", "select <- function(x) x\n"),
    ]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let roots = db.library_roots().roots(&db).clone();
    let packages = roots[0].packages(&db).clone();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name(&db), "dplyr");
    assert_eq!(packages[0].version(&db), &Some("1.0.0".to_string()),);
}

#[cfg(unix)]
#[test]
fn test_scan_library_discovers_symlinked_package() {
    // renv libraries link each package directory into a global cache
    let cache = tempfile::tempdir().unwrap();
    let lib = tempfile::tempdir().unwrap();
    write_package(&cache.path().join("targets"), "targets", &[]);
    std::os::unix::fs::symlink(cache.path().join("targets"), lib.path().join("targets")).unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[lib.path().to_path_buf()]);

    let packages = db.library_roots().roots(&db)[0].packages(&db).clone();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name(&db), "targets");
    assert_eq!(
        packages[0].description_path(&db),
        &FilePath::from_path_buf(lib.path().join("targets").join("DESCRIPTION")).unwrap()
    );
}

#[test]
fn test_package_namespace_reads_from_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg_dir = tmp.path().join("dplyr");
    write_package(&pkg_dir, "dplyr", &[]);
    // The scanner only stats `NAMESPACE`; the lazy `Package::namespace` query
    // is the first thing to actually read and parse it.
    fs::write(
        pkg_dir.join("NAMESPACE"),
        "export(mutate)\nexport(select)\nimportFrom(rlang, sym)\n",
    )
    .unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    let namespace = pkg.namespace(&db);
    assert_eq!(namespace.exports.to_vec(), vec!["mutate", "select"]);
    assert_eq!(namespace.imports, vec![Import {
        name: "sym".to_string(),
        package: "rlang".to_string(),
    }]);
}

#[test]
fn test_package_namespace_empty_when_file_absent() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("dplyr"), "dplyr", &[]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    let namespace = pkg.namespace(&db);
    assert!(namespace.exports.is_empty());
    assert!(namespace.imports.is_empty());
}

#[test]
fn test_package_namespace_rereads_when_revision_bumps() {
    // `Package::namespace` memoizes the parsed `NAMESPACE`. A later disk edit
    // is invisible until something tells salsa the memo is stale, and the only
    // such signal is the `namespace_revision` read inside `namespace()`. So
    // this test rewrites the file, bumps the revision, and checks the second
    // read reflects the edit. Drop that revision read and the assertion below
    // would see the stale `mutate` export.
    let tmp = tempfile::tempdir().unwrap();
    let pkg_dir = tmp.path().join("dplyr");
    write_package(&pkg_dir, "dplyr", &[]);
    fs::write(pkg_dir.join("NAMESPACE"), "export(mutate)\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(pkg.namespace(&db).exports.to_vec(), vec!["mutate"]);

    fs::write(pkg_dir.join("NAMESPACE"), "export(select)\n").unwrap();
    pkg.set_namespace_revision(&mut db)
        .to(oak_db::FileRevision::from(1u128));
    assert_eq!(pkg.namespace(&db).exports.to_vec(), vec!["select"]);
}

#[test]
fn test_package_version_rereads_when_revision_bumps() {
    // Guards the `description_revision` read inside `Package::description`,
    // which `version()` reads through. Same shape as the `NAMESPACE` test:
    // drop the revision read and the second assertion sees the stale `1.0.0`.
    let tmp = tempfile::tempdir().unwrap();
    let pkg_dir = tmp.path().join("dplyr");
    write_package(&pkg_dir, "dplyr", &[]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(pkg.version(&db), &Some("1.0.0".to_string()));

    fs::write(
        pkg_dir.join("DESCRIPTION"),
        "Package: dplyr\nVersion: 2.0.0\n",
    )
    .unwrap();
    pkg.set_description_revision(&mut db)
        .to(oak_db::FileRevision::from(1u128));
    assert_eq!(pkg.version(&db), &Some("2.0.0".to_string()));
}

#[test]
fn test_package_index_rereads_when_revision_bumps() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg_dir = tmp.path().join("dplyr");
    write_package(&pkg_dir, "dplyr", &[]);
    fs::write(pkg_dir.join("INDEX"), "mutate     Mutate columns\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(pkg.index(&db).as_ref().unwrap().names(), &["mutate"]);

    fs::write(pkg_dir.join("INDEX"), "select     Select columns\n").unwrap();
    pkg.set_index_revision(&mut db)
        .to(Some(oak_db::FileRevision::from(1u128)));
    assert_eq!(pkg.index(&db).as_ref().unwrap().names(), &["select"]);
}

#[test]
fn test_reset_same_library_paths_does_not_bump_salsa_revision() {
    // Re-declaring the identical library paths reuses every `Root` entity, so
    // the `LibraryRoots.roots` vec compares equal and the guarded setter is
    // skipped. Salsa has no backdating for inputs, so an unguarded set here
    // would bump the global revision and invalidate everything keyed on the
    // library set. We observe the revision: no change leaves it flat, a real
    // change moves it.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("dplyr"), "dplyr", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let before = salsa::plumbing::current_revision(&db);
    db.set_library_paths(&[tmp.path().to_path_buf()]);
    assert_eq!(salsa::plumbing::current_revision(&db), before);

    // Adding a second path is a real change, so the revision moves. This
    // proves the assertion above isn't vacuously true.
    let tmp2 = tempfile::tempdir().unwrap();
    write_package(&tmp2.path().join("tibble"), "tibble", &[]);
    db.set_library_paths(&[tmp.path().to_path_buf(), tmp2.path().to_path_buf()]);
    assert!(salsa::plumbing::current_revision(&db) > before);
}

#[test]
fn test_scan_multiple_library_paths_preserve_order() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    write_package(&tmp1.path().join("pkg1"), "pkg1", &[]);
    write_package(&tmp2.path().join("pkg2"), "pkg2", &[]);
    let mut db = OakDatabase::new();

    let paths: Vec<PathBuf> = vec![tmp1.path().to_path_buf(), tmp2.path().to_path_buf()];
    db.set_library_paths(&paths);

    let roots = db.library_roots().roots(&db).clone();
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].packages(&db)[0].name(&db), "pkg1");
    assert_eq!(roots[1].packages(&db)[0].name(&db), "pkg2");
}

#[test]
fn test_scan_skips_directory_without_description() {
    let tmp = tempfile::tempdir().unwrap();
    // Looks like a package dir but missing DESCRIPTION.
    fs::create_dir_all(tmp.path().join("not-a-pkg").join("R")).unwrap();
    fs::write(
        tmp.path().join("not-a-pkg").join("R").join("a.R"),
        "x <- 1\n",
    )
    .unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let roots = db.library_roots().roots(&db).clone();
    assert!(roots[0].packages(&db).is_empty());
}

#[test]
fn test_rescan_preserves_root_identity() {
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let root_id_1 = db.library_roots().roots(&db)[0].as_id();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let root_id_2 = db.library_roots().roots(&db)[0].as_id();

    assert_eq!(root_id_1, root_id_2);
}

#[test]
fn test_rescan_preserves_package_identity_by_description_name() {
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "dplyr", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_1 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    // Rescan with no changes on disk.
    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_2 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    assert_eq!(pkg_id_1, pkg_id_2);
}

#[test]
fn test_set_library_paths_unchanged_path_does_not_relist_library() {
    // An unchanged library path is not rescanned by `set_library_paths()`, so
    // the package keeps its original entity and `DESCRIPTION` path after a
    // directory rename. Watcher-driven rescans reconcile such changes.
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("v1"), "mypkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_1 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    fs::rename(tmp.path().join("v1"), tmp.path().join("v2")).unwrap();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];

    assert_eq!(pkg.as_id(), pkg_id_1);
    let v1_description =
        FilePath::from_path_buf(tmp.path().join("v1").join("DESCRIPTION")).unwrap();
    assert_eq!(pkg.description_path(&db), &v1_description);
}

#[test]
fn test_scan_picks_up_collation_field() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg_dir = tmp.path().join("pkg");
    fs::create_dir_all(pkg_dir.join("R")).unwrap();
    fs::write(
        pkg_dir.join("DESCRIPTION"),
        "Package: pkg\nVersion: 1.0.0\nCollate: b.R a.R\n",
    )
    .unwrap();
    mark_installed(&pkg_dir);
    fs::write(pkg_dir.join("R").join("a.R"), "x <- 1\n").unwrap();
    fs::write(pkg_dir.join("R").join("b.R"), "y <- 2\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);

    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(
        pkg.collation(&db),
        &Some(vec!["b.R".to_string(), "a.R".to_string()]),
    );
}

#[test]
fn test_set_library_paths_removed_path_evicts_root() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    write_package(&tmp1.path().join("pkg1"), "pkg1", &[]);
    write_package(&tmp2.path().join("pkg2"), "pkg2", &[]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp1.path().to_path_buf(), tmp2.path().to_path_buf()]);
    assert_eq!(db.library_roots().roots(&db).len(), 2);

    // Drop the second library. The remaining root keeps its identity
    // and its package.
    db.set_library_paths(&[tmp1.path().to_path_buf()]);

    let roots = db.library_roots().roots(&db).clone();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].packages(&db)[0].name(&db), "pkg1");
}

#[test]
fn test_set_library_paths_re_add_preserves_package_identity() {
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("dplyr"), "dplyr", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_before = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    db.set_library_paths(&[]);
    // Package is not reachable via name lookup while the library is removed.
    assert!(db.package_by_name("dplyr").is_none());

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_after = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    assert_eq!(pkg_id_before, pkg_id_after);
    assert!(db.package_by_name("dplyr").is_some());
}

#[test]
fn test_set_library_paths_evicted_package_invisible_to_analysis() {
    // `package_by_name()` uses live membership, not the identity map. An
    // evicted package retains its identity but is invisible to analysis.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("dplyr"), "dplyr", &[]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg = db.package_by_name("dplyr").unwrap();

    db.set_library_paths(&[]);

    assert!(db.package_by_name("dplyr").is_none());
    let description = FilePath::from_path_buf(tmp.path().join("dplyr/DESCRIPTION")).unwrap();
    assert_eq!(db.interned_package(&description), Some(pkg));
}

#[test]
fn test_set_library_paths_keeps_identity_across_cycles() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];

    for _ in 0..3 {
        db.set_library_paths(&[]);
        db.set_library_paths(&[tmp.path().to_path_buf()]);
        assert_eq!(
            db.library_roots().roots(&db)[0].packages(&db).clone(),
            vec![pkg]
        );
    }
}
