use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_package_metadata::namespace::Namespace;
use oak_scan::DbExt;
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

    db.set_library_paths(&[tmp.path().to_path_buf()]);

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
fn test_rescan_preserves_file_identity_by_url() {
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let file_id_1 = db.library_roots().roots(&db)[0].packages(&db)[0].files(&db)[0].as_id();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
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
fn test_set_library_paths_re_add_preserves_file_identity() {
    // The motivating case for `StaleRoot`. Adding, removing, then re-adding
    // a library path returns the same `File` entity for files at the same
    // URL, so downstream salsa caches stay warm and don't bloat on every
    // workspace folder toggle.
    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let file_id_before = db.library_roots().roots(&db)[0].packages(&db)[0].files(&db)[0].as_id();

    db.set_library_paths(&[]);
    // After removal, the file is not reachable through analysis.
    let r_path = tmp.path().join("pkg").join("R").join("a.R");
    let url = UrlId::from_file_path(&r_path).unwrap();
    assert!(db.file_by_url(&url).is_none());

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let file_id_after = db.library_roots().roots(&db)[0].packages(&db)[0].files(&db)[0].as_id();

    assert_eq!(file_id_before, file_id_after);
    // And it's findable again.
    assert!(db.file_by_url(&url).is_some());
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
    // Stale files/packages must not show up in `file_by_url` /
    // `package_by_name`. They're entity-reuse storage, not part of the
    // analysis universe.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("dplyr"), "dplyr", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let r_path = tmp.path().join("dplyr").join("R").join("a.R");
    let url = UrlId::from_file_path(&r_path).unwrap();
    assert!(db.file_by_url(&url).is_some());
    assert!(db.package_by_name("dplyr").is_some());

    db.set_library_paths(&[]);

    // Both lookups miss; the entities are in stale, not in the live universe.
    assert!(db.file_by_url(&url).is_none());
    assert!(db.package_by_name("dplyr").is_none());
    // But the stale buckets do hold them.
    assert_eq!(db.stale_root().files(&db).len(), 1);
    assert_eq!(db.stale_root().packages(&db).len(), 1);
}

#[test]
fn test_set_library_paths_stale_no_duplicates_across_cycles() {
    // Repeated add/remove/add must not duplicate entities in stale: on
    // re-add the entity comes back out of stale, so by the time we
    // remove it again there's only one copy to push back in.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    for _ in 0..3 {
        db.set_library_paths(&[tmp.path().to_path_buf()]);
        db.set_library_paths(&[]);
    }

    let stale = db.stale_root();
    assert_eq!(stale.files(&db).len(), 1);
    assert_eq!(stale.packages(&db).len(), 1);
}

#[test]
fn test_set_library_paths_resurrected_file_picks_up_disk_contents() {
    // Eviction doesn't snapshot disk contents. When a file is
    // resurrected from stale, it should reflect the current disk state,
    // not whatever it had when evicted.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "v1\n")]);
    let mut db = OakDatabase::new();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    db.set_library_paths(&[]);

    let r_path = tmp.path().join("pkg").join("R").join("a.R");
    fs::write(&r_path, "v2\n").unwrap();

    db.set_library_paths(&[tmp.path().to_path_buf()]);
    let url = UrlId::from_file_path(&r_path).unwrap();
    let file = db.file_by_url(&url).unwrap();
    assert_eq!(file.contents(&db), "v2\n");
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

fn file_url(s: &str) -> UrlId {
    // Bypass `UrlId::from_file_path`'s canonicalization (these paths
    // don't exist on disk).
    UrlId::from_canonical(Url::parse(&format!("file://{s}")).unwrap())
}

fn empty_library_root(db: &OakDatabase, path: &str) -> Root {
    Root::new(db, file_url(path), RootKind::Library, vec![], vec![])
}

/// Stash `pkg` in `root.packages` and register `root` on
/// `library_roots`, mirroring what the library scanner does after
/// `set_package` returns. Without this step `package_by_url` can't see
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
    let desc_url = file_url("/lib/sub/DESCRIPTION");

    let p1 = short.set_package(
        &mut db,
        desc_url.clone(),
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    register_package(&mut db, short, p1);
    assert_eq!(p1.root(&db), short);

    let p2 = long.set_package(
        &mut db,
        desc_url,
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    // Same entity, re-rooted to the more-specific root.
    assert_eq!(p1, p2);
    assert_eq!(p1.root(&db), long);
}

#[test]
fn test_set_package_shorter_root_does_not_steal_from_longer() {
    let mut db = OakDatabase::new();
    let short = empty_library_root(&db, "/lib");
    let long = empty_library_root(&db, "/lib/sub");
    let desc_url = file_url("/lib/sub/DESCRIPTION");

    let p1 = long.set_package(
        &mut db,
        desc_url.clone(),
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    register_package(&mut db, long, p1);
    assert_eq!(p1.root(&db), long);

    let p2 = short.set_package(
        &mut db,
        desc_url,
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    // Same entity, but the backpointer keeps the longer root.
    assert_eq!(p1, p2);
    assert_eq!(p1.root(&db), long);
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
    let r_url = file_url("/lib/pkg/R/a.R");
    let file = File::new(&db, r_url.clone(), "editor content".to_string(), None);
    db.orphan_root().set_files(&mut db).to(vec![file]);
    assert_eq!(file.package(&db), None);

    // Now a library scan picks up the same URL as part of a package.
    let lib = empty_library_root(&db, "/lib");
    let pkg = lib.set_package(
        &mut db,
        file_url("/lib/pkg/DESCRIPTION"),
        "pkg".to_string(),
        None,
        Namespace::default(),
        vec![FileEntry {
            url: r_url.clone(),
            contents: "disk content".to_string(),
        }],
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
fn test_set_package_equal_depth_updates_root() {
    // Stale resurrection: the previous Root entity at this path was
    // evicted (so it's no longer in `library_roots`), and a fresh Root
    // is created at the same path. The resurrected Package's `root`
    // field still names the old entity, so `set_package` on the new
    // one must re-root.
    let mut db = OakDatabase::new();
    let old = empty_library_root(&db, "/lib");
    let new = empty_library_root(&db, "/lib");
    assert_ne!(old, new);
    let desc_url = file_url("/lib/pkg/DESCRIPTION");

    let p1 = old.set_package(
        &mut db,
        desc_url.clone(),
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    // Simulate eviction: drop `old` from `library_roots` and move the
    // package into `stale_root.packages`.
    db.stale_root().set_packages(&mut db).to(vec![p1]);
    db.library_roots().set_roots(&mut db).to(vec![]);
    assert_eq!(p1.root(&db), old);

    let p2 = new.set_package(
        &mut db,
        desc_url,
        "pkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    assert_eq!(p1, p2);
    assert_eq!(p1.root(&db), new);
}
