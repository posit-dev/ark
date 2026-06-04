use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_package_metadata::namespace::Namespace;
use oak_scan::DbScan;
use oak_scan::RootExt;
use salsa::Setter;
use url::Url;

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
fn test_rescan_renamed_package_dir_keeps_package_identity() {
    // Identity is `(root, DESCRIPTION name)`, not the directory path.
    // Renaming the package directory but keeping the same DESCRIPTION
    // name (and same library root) preserves the salsa `Package` id.
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("v1"), "mypkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let pkg_id_1 = db.library_roots().roots(&db)[0].packages(&db)[0].as_id();

    // Rename the package directory. DESCRIPTION still says `mypkg`.
    fs::rename(tmp.path().join("v1"), tmp.path().join("v2")).unwrap();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
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
    write_package(&tmp.path().join("pkg"), "dplyr", &[("a.R", "x <- 1\n")]);
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
fn test_set_library_paths_stale_invisible_to_analysis() {
    // A stale package must not show up in `package_by_name`. Stale is
    // entity-reuse storage, not part of the analysis universe.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("dplyr"), "dplyr", &[]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    assert!(db.package_by_name("dplyr").is_some());

    db.set_library_paths(&[]);

    // The lookup misses; the package is in stale, not the live universe.
    assert!(db.package_by_name("dplyr").is_none());
    assert_eq!(db.stale_root().packages(&db).len(), 1);
}

#[test]
fn test_set_library_paths_stale_no_duplicates_across_cycles() {
    // Repeated add/remove/add must not duplicate entities in stale: on
    // re-add the entity comes back out of stale, so by the time we
    // remove it again there's only one copy to push back in.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[]);
    let mut db = OakDatabase::new();

    for _ in 0..3 {
        db.set_library_paths(&[tmp.path().to_path_buf()]);
        db.set_library_paths(&[]);
    }

    assert_eq!(db.stale_root().packages(&db).len(), 1);
}

// --- nested-root resolution (longest-path-wins) ---
//
// The library scanner walks depth-1 so it can't naturally produce two
// library roots that both claim the same DESCRIPTION (a nested library
// root at `/lib/foo` would walk children of `/lib/foo`, not
// `/lib/foo/DESCRIPTION` itself). These tests exercise `RootExt::set_package`
// directly with hand-built `Root` entities so they cover the shared
// resolution logic that PR 12's workspace scanner -- which walks any depth
// and is the realistic trigger -- depends on.

fn file_path(s: &str) -> FilePath {
    FilePath::from_url(&Url::parse(&format!("file://{s}")).unwrap())
}

fn empty_library_root(db: &OakDatabase, path: &str) -> Root {
    Root::new(db, file_path(path), RootKind::Library, vec![], vec![])
}

/// Stash `pkg` in `root.packages` and register `root` on
/// `library_roots`, mirroring what the library scanner does after
/// `set_package` returns. Without this step `package_by_path` can't see
/// the package on subsequent `set_package` calls.
fn register_package(db: &mut OakDatabase, root: Root, pkg: Package) {
    root.set_packages(db).to(vec![pkg]);
    let mut roots = db.library_roots().roots(db).clone();
    if !roots.contains(&root) {
        roots.push(root);
    }
    db.library_roots().set_roots(db).to(roots);
}

#[test]
fn test_set_package_longer_root_wins_after_shorter_claims_first() {
    let mut db = OakDatabase::new();
    let short = empty_library_root(&db, "/lib");
    let long = empty_library_root(&db, "/lib/sub");
    let desc_path = file_path("/lib/sub/DESCRIPTION");

    let p1 = short.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );
    register_package(&mut db, short, p1);
    assert_eq!(db.root_by_package(p1), Some(short));

    let p2 = long.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );
    register_package(&mut db, long, p2);
    // Same entity; now in both roots' `packages`. `root_by_package` prefers
    // the longer (more specific) root.
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(long));
}

#[test]
fn test_set_package_shorter_root_does_not_steal_from_longer() {
    let mut db = OakDatabase::new();
    let short = empty_library_root(&db, "/lib");
    let long = empty_library_root(&db, "/lib/sub");
    let desc_path = file_path("/lib/sub/DESCRIPTION");

    let p1 = long.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );
    register_package(&mut db, long, p1);
    assert_eq!(db.root_by_package(p1), Some(long));

    let p2 = short.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );
    register_package(&mut db, short, p2);
    // Same entity; now in both roots' `packages`. `root_by_package` keeps the
    // longer root as the owner.
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(long));
}

#[test]
fn test_all_files_emits_shared_file_once_under_deepest_root() {
    use oak_db::all_files;
    use oak_scan::FileEntry;

    let mut db = OakDatabase::new();
    let short = empty_library_root(&db, "/lib");
    let long = empty_library_root(&db, "/lib/sub");
    let desc_url = file_url("/lib/sub/DESCRIPTION");
    let r_url = file_url("/lib/sub/R/a.R");
    let files = vec![FileEntry {
        url: r_url,
        contents: "f <- function() NULL\n".to_string(),
    }];

    // Same DESCRIPTION scanned from both roots reuses one `Package`, so
    // both roots' `packages` vecs list it and its files are reachable from
    // both. The deepest root owns them.
    let p1 = short.set_package(
        &mut db,
        desc_url.clone(),
        "pkg".to_string(),
        None,
        Namespace::default(),
        files.clone(),
        Vec::new(),
        None,
    );
    register_package(&mut db, short, p1);
    let p2 = long.set_package(
        &mut db,
        desc_url,
        "pkg".to_string(),
        None,
        Namespace::default(),
        files,
        Vec::new(),
        None,
    );
    register_package(&mut db, long, p2);
    assert_eq!(p1, p2);

    let file = p1.files(&db)[0];
    assert_eq!(file.root(&db), Some(long));
    assert_eq!(all_files(&db), &vec![file]);
}

#[test]
fn test_upsert_re_promotes_editor_owned_file_from_orphan() {
    // Mirrors the doc claim in `eviction.rs`: an editor-open file that
    // was evicted to `OrphanRoot` should come back into `pkg.files`
    // when the package's root is re-added. Same `File` entity, editor
    // contents preserved (the scan's disk snapshot doesn't overwrite),
    // and the orphan reference is cleaned up so the orphan-placement
    // invariant (`package == None`) stays honest.
    use oak_db::File;
    use oak_scan::FileEntry;

    let mut db = OakDatabase::new();

    // Editor opens the file before any scan -> orphan.
    let r_path = file_path("/lib/pkg/R/a.R");
    let file = File::new(&db, r_path.clone(), "editor content".to_string(), None);
    db.orphan_root()
        .set_files(&mut db)
        .to(HashSet::from([file]));
    assert_eq!(file.package(&db), None);

    // Now a library scan picks up the same URL as part of a package.
    let lib = empty_library_root(&db, "/lib");
    let pkg = lib.set_package(
        &mut db,
        file_path("/lib/pkg/DESCRIPTION"),
        "pkg".to_string(),
        None,
        Namespace::default(),
        vec![FileEntry {
            path: r_path.clone(),
            contents: "disk content".to_string(),
        }],
        Vec::new(),
        None,
    );

    // Same `File` entity, editor content preserved, package backpointer set.
    let pkg_file = pkg.files(&db)[0];
    assert_eq!(pkg_file, file);
    assert_eq!(file.contents(&db), "editor content");
    assert_eq!(file.package(&db), Some(pkg));

    // Orphan reference cleaned up.
    assert!(!db.orphan_root().files(&db).contains(&file));
}

#[test]
fn test_set_package_stale_resurrection_changes_owning_root() {
    // Stale resurrection: the previous `Root` entity at this path was
    // evicted (so it's no longer in `library_roots`), and a fresh `Root`
    // is created at the same path. The resurrected `Package` ends up in
    // the new root's `packages` vec, and `root_by_package` reports the new
    // root accordingly.
    let mut db = OakDatabase::new();
    let old = empty_library_root(&db, "/lib");
    let new = empty_library_root(&db, "/lib");
    assert_ne!(old, new);
    let desc_path = file_path("/lib/pkg/DESCRIPTION");

    let p1 = old.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );
    register_package(&mut db, old, p1);
    assert_eq!(db.root_by_package(p1), Some(old));

    // Simulate eviction: drop `old` from `library_roots` and move the
    // package into `stale_root.packages`.
    db.stale_root().set_packages(&mut db).to(vec![p1]);
    db.library_roots().set_roots(&mut db).to(vec![]);
    assert_eq!(db.root_by_package(p1), None);

    let p2 = new.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );
    register_package(&mut db, new, p2);
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(new));
}
