use std::fs;
use std::path::Path;

use oak_db::Db;
use oak_db::File;
use oak_db::OakDatabase;

use crate::DbScan;

/// Write an installed package's `DESCRIPTION` under `lib`
fn write_description(lib: &Path, name: &str, collate: Option<&[&str]>) {
    let directory = lib.join(name);
    fs::create_dir_all(&directory).unwrap();

    let mut description = format!("Package: {name}\nVersion: 1.0.0\n");

    if let Some(collate) = collate {
        description.push_str("Collate:");
        for basename in collate {
            description.push_str(&format!(" '{basename}'"));
        }
        description.push('\n');
    }

    fs::write(directory.join("DESCRIPTION"), description).unwrap();
}

/// Write `*.R` files directly under `directory`, the layout a source provider hands to
/// `set_package_sources`.
fn write_source_directory(directory: &Path, r_files: &[(&str, &str)]) {
    fs::create_dir_all(directory).unwrap();
    for (basename, contents) in r_files {
        fs::write(directory.join(basename), contents).unwrap();
    }
}

fn basenames(db: &OakDatabase, files: &[File]) -> Vec<String> {
    files
        .iter()
        .map(|file| {
            file.path(db)
                .to_url()
                .path()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn test_set_package_sources_alphabetical() {
    let lib = tempfile::tempdir().unwrap();
    write_description(lib.path(), "mypkg", None);

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let pkg = db.package_by_name("mypkg").unwrap();
    assert!(pkg.files(&db).is_empty());

    let src = tempfile::tempdir().unwrap();
    write_source_directory(src.path(), &[("b.R", "b <- 1\n"), ("a.R", "a <- 1\n")]);
    db.set_package_sources(pkg, src.path());

    let files = pkg.files(&db).clone();
    assert_eq!(basenames(&db, &files), ["a.R", "b.R"]);
    assert!(pkg.scripts(&db).is_empty());

    // Each file carries the package backpointer (path-based containment can't
    // reach a cache dir outside the library root, so resolution relies on it).
    for file in &files {
        assert_eq!(file.package(&db), Some(pkg));
    }

    // Source is read lazily from disk via the revision, not an editor override.
    assert_eq!(files[0].source_text(&db), "a <- 1\n");
}

#[test]
fn test_set_package_sources_collation_order() {
    let lib = tempfile::tempdir().unwrap();
    write_description(lib.path(), "mypkg", Some(&["b.R", "a.R"]));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let pkg = db.package_by_name("mypkg").unwrap();
    assert!(pkg.files(&db).is_empty());

    let src = tempfile::tempdir().unwrap();
    write_source_directory(src.path(), &[("a.R", "a <- 1\n"), ("b.R", "b <- 1\n")]);
    db.set_package_sources(pkg, src.path());

    // Collation order preserved!
    assert_eq!(basenames(&db, &pkg.files(&db).clone()), ["b.R", "a.R"]);
    assert!(pkg.scripts(&db).is_empty());
}

#[test]
fn test_set_package_sources_leftover_becomes_scripts() {
    let lib = tempfile::tempdir().unwrap();
    write_description(lib.path(), "mypkg", Some(&["a.R"]));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let pkg = db.package_by_name("mypkg").unwrap();
    assert!(pkg.files(&db).is_empty());

    let src = tempfile::tempdir().unwrap();
    write_source_directory(src.path(), &[("a.R", "a <- 1\n"), ("c.R", "c <- 1\n")]);
    db.set_package_sources(pkg, src.path());

    // `c.R` isn't in `Collate:`, so R won't load it. It ends up in `scripts`.
    assert_eq!(basenames(&db, &pkg.files(&db).clone()), ["a.R"]);
    assert_eq!(basenames(&db, &pkg.scripts(&db).clone()), ["c.R"]);
}
