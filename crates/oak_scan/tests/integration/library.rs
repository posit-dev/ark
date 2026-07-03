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
use oak_package_metadata::namespace::Import;
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
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, short, p1);
    assert_eq!(db.root_by_package(p1), Some(short));

    let p2 = long.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        Vec::new(),
        Vec::new(),
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
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, long, p1);
    assert_eq!(db.root_by_package(p1), Some(long));

    let p2 = short.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        Vec::new(),
        Vec::new(),
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
    let desc_path = file_path("/lib/sub/DESCRIPTION");
    let r_path = file_path("/lib/sub/R/a.R");
    let files = vec![FileEntry {
        path: r_path,
        revision: oak_db::FileRevision::zero(),
    }];

    // Same DESCRIPTION scanned from both roots reuses one `Package`, so
    // both roots' `packages` vecs list it and its files are reachable from
    // both. The deepest root owns them.
    let p1 = short.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        files.clone(),
        Vec::new(),
    );
    register_package(&mut db, short, p1);
    let p2 = long.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        files,
        Vec::new(),
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
    let file = File::new(
        &db,
        r_path.clone(),
        oak_db::FileRevision::zero(),
        Some("editor content".to_string()),
        None,
    );
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
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        vec![FileEntry {
            path: r_path.clone(),
            revision: oak_db::FileRevision::zero(),
        }],
        Vec::new(),
    );

    // Same `File` entity, editor content preserved, package backpointer set.
    let pkg_file = pkg.files(&db)[0];
    assert_eq!(pkg_file, file);
    assert_eq!(file.source_text(&db), "editor content");
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
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        Vec::new(),
        Vec::new(),
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
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, new, p2);
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(new));
}
