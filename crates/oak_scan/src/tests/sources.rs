use std::fs;
use std::path::Path;

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::SourceDb;
use oak_package_metadata::tests::mark_installed;

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
    mark_installed(&directory);
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

    let files = pkg.files(&db).to_vec();
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
    assert_eq!(basenames(&db, pkg.files(&db)), ["b.R", "a.R"]);
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
    assert_eq!(basenames(&db, pkg.files(&db)), ["a.R"]);
    assert_eq!(basenames(&db, pkg.scripts(&db)), ["c.R"]);
}

/// A library holding `base`, registered without files, as the library scanner does.
fn library_with_base(collate: Option<&[&str]>) -> (tempfile::TempDir, OakDatabase, Package) {
    let lib = tempfile::tempdir().unwrap();
    write_description(lib.path(), "base", collate);

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let pkg = db.package_by_name("base").unwrap();
    (lib, db, pkg)
}

fn base_sources(r_files: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write_source_directory(directory.path(), r_files);
    directory
}

fn primitive_stubs() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write_source_directory(directory.path(), &[(
        "primitives.R",
        "sum <- function(...) NULL\n",
    )]);
    directory
}

#[test]
fn test_generated_files_follow_sources_in_either_order() {
    let sources = base_sources(&[("b.R", "b <- 1\n"), ("a.R", "a <- 1\n")]);
    let stubs = primitive_stubs();

    let paths = |db: &OakDatabase, pkg: Package| -> Vec<FilePath> {
        pkg.files(db)
            .iter()
            .map(|file| file.path(db).clone())
            .collect()
    };

    let (_lib, mut db, pkg) = library_with_base(None);
    db.set_package_sources(pkg, sources.path());
    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());
    let sources_first = paths(&db, pkg);

    let (_lib, mut db, pkg) = library_with_base(None);
    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());
    assert_eq!(basenames(&db, pkg.files(&db)), ["primitives.R"]);
    db.set_package_sources(pkg, sources.path());

    assert_eq!(paths(&db, pkg), sources_first);
    assert_eq!(basenames(&db, pkg.files(&db)), [
        "a.R",
        "b.R",
        "primitives.R"
    ]);
    assert!(pkg.scripts(&db).is_empty());

    for file in pkg.files(&db) {
        assert_eq!(file.package(&db), Some(pkg));
    }
}

#[test]
fn test_redelivered_files_write_nothing() {
    let sources = base_sources(&[("a.R", "a <- 1\n")]);
    let stubs = primitive_stubs();

    let (_lib, mut db, pkg) = library_with_base(None);
    db.set_package_sources(pkg, sources.path());
    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());

    let before = salsa::plumbing::current_revision(&db);
    db.set_package_sources(pkg, sources.path());
    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());
    assert_eq!(salsa::plumbing::current_revision(&db), before);
}

#[test]
fn test_source_refetch_keeps_generated_files() {
    let stubs = primitive_stubs();

    let (_lib, mut db, pkg) = library_with_base(None);
    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());
    let stub = pkg.files(&db)[0];

    let sources = base_sources(&[("a.R", "a <- 1\n")]);
    db.set_package_sources(pkg, sources.path());
    let refetched = base_sources(&[("b.R", "b <- 1\n"), ("c.R", "c <- 1\n")]);
    db.set_package_sources(pkg, refetched.path());

    assert_eq!(basenames(&db, pkg.files(&db)), [
        "b.R",
        "c.R",
        "primitives.R"
    ]);
    assert_eq!(pkg.files(&db)[2], stub);
}

#[test]
fn test_generated_files_leave_scripts_alone() {
    let sources = base_sources(&[("a.R", "a <- 1\n"), ("c.R", "c <- 1\n")]);
    let stubs = primitive_stubs();

    // `Collate:` excludes `c.R` from the namespace but does not exclude generated stubs.
    let (_lib, mut db, pkg) = library_with_base(Some(&["a.R"]));
    db.set_package_sources(pkg, sources.path());
    let scripts = pkg.scripts(&db).clone();
    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());
    assert_eq!(basenames(&db, pkg.files(&db)), ["a.R", "primitives.R"]);
    assert_eq!(pkg.scripts(&db), &scripts);
    assert_eq!(basenames(&db, pkg.scripts(&db)), ["c.R"]);

    let (_lib, mut db, pkg) = library_with_base(None);
    db.set_package_sources(pkg, sources.path());
    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());
    assert_eq!(basenames(&db, pkg.files(&db)), [
        "a.R",
        "c.R",
        "primitives.R"
    ]);
    assert!(pkg.scripts(&db).is_empty());
}

#[test]
fn test_eviction_clears_generated_files() {
    let sources = base_sources(&[("a.R", "a <- 1\n")]);
    let stubs = primitive_stubs();

    let (lib, mut db, pkg) = library_with_base(None);
    db.set_package_sources(pkg, sources.path());
    db.set_package_facts(pkg, Some(stubs.path()), vec![String::from("obj")]);
    let files = pkg.files(&db).to_vec();
    assert!(pkg.facts_revision(&db).is_some());
    assert_eq!(pkg.lazy_data(&db), ["obj"]);

    db.set_library_paths(&[]);
    assert!(db.package_by_name("base").is_none());
    assert!(pkg.files(&db).is_empty());
    assert_eq!(pkg.facts_revision(&db), None);
    assert!(pkg.lazy_data(&db).is_empty());
    for file in &files {
        assert_eq!(file.package(&db), None);
    }

    db.set_library_paths(&[lib.path().to_path_buf()]);
    assert_eq!(db.package_by_name("base"), Some(pkg));
    assert!(pkg.files(&db).is_empty());
    assert_eq!(pkg.facts_revision(&db), None);
    assert!(pkg.lazy_data(&db).is_empty());

    db.set_package_facts(pkg, Some(stubs.path()), Vec::new());
    db.set_package_sources(pkg, sources.path());
    assert_eq!(pkg.files(&db), files);
    for file in &files {
        assert_eq!(file.package(&db), Some(pkg));
    }
}

#[test]
fn test_set_package_sources_keeps_dropped_open_buffer_visible() {
    let lib = tempfile::tempdir().unwrap();
    write_description(lib.path(), "mypkg", None);

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let pkg = db.package_by_name("mypkg").unwrap();

    let src = tempfile::tempdir().unwrap();
    write_source_directory(src.path(), &[("a.R", "a <- 1\n"), ("b.R", "b <- 1\n")]);
    db.set_package_sources(pkg, src.path());
    let path = FilePath::from_path_buf(src.path().join("b.R")).unwrap();
    let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());

    fs::remove_file(src.path().join("b.R")).unwrap();
    db.set_package_sources(pkg, src.path());

    assert_eq!(basenames(&db, pkg.files(&db)), ["a.R"]);
    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
}
