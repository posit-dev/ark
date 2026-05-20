use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::RootKind;
use oak_scan::DbExt;

/// Write a minimal R package layout under `dir`: a `DESCRIPTION` file
/// with `Package: {name}, Version: 1.0.0`, plus the `R/` files in
/// `r_files` (basename -> contents).
fn write_package(dir: &Path, name: &str, r_files: &[(&str, &str)]) {
    fs::create_dir_all(dir.join("R")).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 1.0.0\n"),
    )
    .unwrap();
    for (basename, contents) in r_files {
        fs::write(dir.join("R").join(basename), contents).unwrap();
    }
}

#[test]
fn test_scan_empty_library_path_registers_empty_root() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);

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

    db.scan_library_paths(&[tmp.path().to_path_buf()]);

    let roots = db.library_roots().roots(&db).clone();
    let packages = roots[0].packages(&db).clone();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name(&db), "dplyr");
    assert_eq!(packages[0].version(&db), &Some("1.0.0".to_string()),);

    let files = packages[0].files(&db).clone();
    assert_eq!(files.len(), 2);
    // Alphabetical by basename: mutate.R, select.R.
    assert!(files[0].url(&db).as_url().path().ends_with("mutate.R"));
    assert!(files[1].url(&db).as_url().path().ends_with("select.R"));
}

#[test]
fn test_scan_library_files_are_findable_by_url() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);

    let r_path = tmp.path().join("pkg").join("R").join("a.R");
    let url = UrlId::from_file_path(&r_path).unwrap();
    let file = db
        .file_by_url(&url)
        .expect("scanned file should be findable");
    assert_eq!(file.contents(&db), "x <- 1\n");
}

#[test]
fn test_scan_multiple_library_paths_preserve_order() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    write_package(&tmp1.path().join("pkg1"), "pkg1", &[]);
    write_package(&tmp2.path().join("pkg2"), "pkg2", &[]);
    let mut db = OakDatabase::new();

    let paths: Vec<PathBuf> = vec![tmp1.path().to_path_buf(), tmp2.path().to_path_buf()];
    db.scan_library_paths(&paths);

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

    db.scan_library_paths(&[tmp.path().to_path_buf()]);

    let roots = db.library_roots().roots(&db).clone();
    assert!(roots[0].packages(&db).is_empty());
}

#[test]
fn test_rescan_preserves_root_identity() {
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let root_id_1 = db.library_roots().roots(&db)[0].as_id();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let root_id_2 = db.library_roots().roots(&db)[0].as_id();

    assert_eq!(root_id_1, root_id_2);
}

#[test]
fn test_rescan_preserves_package_identity_by_description_name() {
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "dplyr", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_1 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    // Rescan with no changes on disk.
    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_2 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    assert_eq!(pkg_id_1, pkg_id_2);
}

#[test]
fn test_rescan_preserves_file_identity_by_url() {
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let file_id_1 = db.library_roots().roots(&db)[0].packages(&db)[0].files(&db)[0].as_id();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let file_id_2 = db.library_roots().roots(&db)[0].packages(&db)[0].files(&db)[0].as_id();

    assert_eq!(file_id_1, file_id_2);
}

#[test]
fn test_rescan_renamed_package_dir_keeps_package_identity() {
    // Identity is `(root, DESCRIPTION name)`, not the directory path.
    // Renaming the package directory but keeping the same DESCRIPTION
    // name (and same library root) preserves the salsa `Package` id.
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("v1"), "mypkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_1 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    // Rename the package directory. DESCRIPTION still says `mypkg`.
    fs::rename(tmp.path().join("v1"), tmp.path().join("v2")).unwrap();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_2 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    assert_eq!(pkg_id_1, pkg_id_2);
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
    fs::write(pkg_dir.join("R").join("a.R"), "x <- 1\n").unwrap();
    fs::write(pkg_dir.join("R").join("b.R"), "y <- 2\n").unwrap();
    let mut db = OakDatabase::new();

    db.scan_library_paths(&[tmp.path().to_path_buf()]);

    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(
        pkg.collation(&db),
        &Some(vec!["b.R".to_string(), "a.R".to_string()]),
    );
}
