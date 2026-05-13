use std::path::Path;

use crate::vfs_scan::scan;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn scan_finds_package_with_default_collation() {
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");
    write(
        &pkg_root.join("DESCRIPTION"),
        "Package: mypkg\nVersion: 0.0.1\n",
    );
    write(&pkg_root.join("NAMESPACE"), "export(foo)\n");
    write(&pkg_root.join("R/aaa.R"), "foo <- 1\n");
    write(&pkg_root.join("R/zzz.R"), "bar <- 2\n");

    let result = scan(&[dir.path().to_path_buf()]);

    assert_eq!(result.packages.len(), 1);
    let pkg = &result.packages[0];
    assert_eq!(pkg.name, "mypkg");
    assert!(pkg.collation_spec.is_none());
    assert_eq!(pkg.files.len(), 2);
    assert_eq!(pkg.files[0].path, pkg_root.join("R/aaa.R"));
    assert_eq!(pkg.files[1].path, pkg_root.join("R/zzz.R"));
    assert_eq!(pkg.files[0].contents, "foo <- 1\n");
    assert!(result.scripts.is_empty());
}

#[test]
fn scan_respects_collate_field_order() {
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");
    write(
        &pkg_root.join("DESCRIPTION"),
        "Package: mypkg\nVersion: 0.0.1\nCollate: zzz.R aaa.R\n",
    );
    write(&pkg_root.join("R/aaa.R"), "a <- 1\n");
    write(&pkg_root.join("R/zzz.R"), "z <- 2\n");

    let result = scan(&[dir.path().to_path_buf()]);

    let pkg = &result.packages[0];
    assert_eq!(
        pkg.collation_spec.as_deref(),
        Some(&["zzz.R".to_string(), "aaa.R".to_string()][..])
    );
    assert_eq!(pkg.files.len(), 2);
    assert_eq!(pkg.files[0].path, pkg_root.join("R/zzz.R"));
    assert_eq!(pkg.files[1].path, pkg_root.join("R/aaa.R"));
}

#[test]
fn scan_collects_scripts_outside_packages() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("analysis.R"), "x <- 1\n");
    write(&dir.path().join("subdir/other.R"), "y <- 2\n");

    let result = scan(&[dir.path().to_path_buf()]);

    assert!(result.packages.is_empty());
    assert_eq!(result.scripts.len(), 2);
    let paths: Vec<_> = result.scripts.iter().map(|s| &s.path).collect();
    assert!(paths.contains(&&dir.path().join("analysis.R")));
    assert!(paths.contains(&&dir.path().join("subdir/other.R")));
}

#[test]
fn scan_excludes_package_files_from_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");
    write(
        &pkg_root.join("DESCRIPTION"),
        "Package: mypkg\nVersion: 0.0.1\n",
    );
    write(&pkg_root.join("R/foo.R"), "foo <- 1\n");

    write(&dir.path().join("script.R"), "x <- 1\n");

    let result = scan(&[dir.path().to_path_buf()]);

    assert_eq!(result.packages.len(), 1);
    assert_eq!(result.scripts.len(), 1);
    assert_eq!(result.scripts[0].path, dir.path().join("script.R"));
}

#[test]
fn scan_skips_renv_directory() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("renv/activate.R"), "renv_setup()\n");
    write(&dir.path().join("script.R"), "x <- 1\n");

    let result = scan(&[dir.path().to_path_buf()]);

    assert_eq!(result.scripts.len(), 1);
    assert_eq!(result.scripts[0].path, dir.path().join("script.R"));
}

#[test]
fn scan_skips_gitignored_files() {
    let dir = tempfile::tempdir().unwrap();
    // `.gitignore` only takes effect inside a git repo for the `ignore`
    // crate's default settings, mirroring real editor workspaces.
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    write(&dir.path().join(".gitignore"), "ignored/\n");
    write(&dir.path().join("ignored/skip.R"), "skip\n");
    write(&dir.path().join("kept.R"), "keep\n");

    let result = scan(&[dir.path().to_path_buf()]);

    assert_eq!(result.scripts.len(), 1);
    assert_eq!(result.scripts[0].path, dir.path().join("kept.R"));
}

#[test]
fn scan_returns_empty_for_empty_root() {
    let dir = tempfile::tempdir().unwrap();
    let result = scan(&[dir.path().to_path_buf()]);
    assert!(result.packages.is_empty());
    assert!(result.scripts.is_empty());
}
