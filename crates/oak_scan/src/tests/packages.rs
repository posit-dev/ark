//! Tests for [`read_package_sources`], which decides which R files a package
//! contributes to its loadable namespace and which are standalone scripts.

use std::fs;
use std::path::Path;

use crate::inputs::FileEntry;
use crate::packages::read_package_sources;

/// Write `files` into `dir`, creating it first.
fn write_r_dir(dir: &Path, files: &[(&str, &str)]) {
    fs::create_dir_all(dir).unwrap();
    for (basename, contents) in files {
        fs::write(dir.join(basename), contents).unwrap();
    }
}

/// Basenames of `files`, in the order returned.
fn names(files: &[FileEntry]) -> Vec<String> {
    files
        .iter()
        .map(|file| file.path.file_name().unwrap().into_owned())
        .collect()
}

/// A data-only package has no `R/` at all. `datasets` is on the default search
/// path, so this is a routine state and must yield no files rather than an
/// error.
#[test]
fn test_missing_r_directory_yields_no_files() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("datasets").join("R");

    let (files, scripts) = read_package_sources(&missing, None);

    assert!(files.is_empty());
    assert!(scripts.is_empty());
}

/// An `R/` that exists but holds no R code is distinct from one that's absent,
/// and lands in the same place.
#[test]
fn test_empty_r_directory_yields_no_files() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("R");
    write_r_dir(&r, &[]);

    let (files, scripts) = read_package_sources(&r, None);

    assert!(files.is_empty());
    assert!(scripts.is_empty());
}

/// Without `Collate:`, every R file is loadable, ordered case-insensitively by
/// basename so the result doesn't depend on `read_dir` order.
#[test]
fn test_without_collation_all_files_are_loadable_and_sorted() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("R");
    write_r_dir(&r, &[("zebra.R", "1"), ("Apple.R", "1"), ("mango.R", "1")]);

    let (files, scripts) = read_package_sources(&r, None);

    assert_eq!(names(&files), vec!["Apple.R", "mango.R", "zebra.R"]);
    assert!(scripts.is_empty());
}

/// Non-R files and subdirectories are skipped. `R/` is loaded as a flat
/// directory, so `R/sub/nested.R` isn't part of the namespace.
#[test]
fn test_non_r_entries_and_subdirectories_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("R");
    write_r_dir(&r, &[
        ("keep.R", "1"),
        ("README.md", "x"),
        ("data.csv", "x"),
    ]);
    write_r_dir(&r.join("sub"), &[("nested.R", "1")]);

    let (files, scripts) = read_package_sources(&r, None);

    assert_eq!(names(&files), vec!["keep.R"]);
    assert!(scripts.is_empty());
}

/// `Collate:` sets the load order, which is not alphabetical. Files it lists
/// become the namespace in exactly that sequence.
#[test]
fn test_collation_sets_load_order() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("R");
    write_r_dir(&r, &[("a.R", "1"), ("b.R", "1"), ("c.R", "1")]);

    let order = ["c.R".to_string(), "a.R".to_string(), "b.R".to_string()];
    let (files, scripts) = read_package_sources(&r, Some(&order));

    assert_eq!(names(&files), vec!["c.R", "a.R", "b.R"]);
    assert!(scripts.is_empty());
}

/// A file on disk that `Collate:` omits can't enter the namespace, so it's kept
/// as a standalone script rather than dropped. Leftovers are sorted.
#[test]
fn test_files_absent_from_collation_become_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("R");
    write_r_dir(&r, &[("listed.R", "1"), ("zebra.R", "1"), ("apple.R", "1")]);

    let order = ["listed.R".to_string()];
    let (files, scripts) = read_package_sources(&r, Some(&order));

    assert_eq!(names(&files), vec!["listed.R"]);
    assert_eq!(names(&scripts), vec!["apple.R", "zebra.R"]);
}

/// A `Collate:` entry with no file on disk is skipped rather than fabricated.
#[test]
fn test_collation_entry_without_a_file_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("R");
    write_r_dir(&r, &[("present.R", "1")]);

    let order = ["missing.R".to_string(), "present.R".to_string()];
    let (files, scripts) = read_package_sources(&r, Some(&order));

    assert_eq!(names(&files), vec!["present.R"]);
    assert!(scripts.is_empty());
}
