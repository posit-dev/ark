//! Nested-root resolution through [`RootExt`] and open-buffer visibility
//! across library eviction.

use std::fs;
use std::path::Path;

use aether_path::FilePath;
use oak_db::all_known_files;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_db::SourceDb;
use salsa::Setter;
use url::Url;

use crate::inputs::FileEntry;
use crate::inputs::RootExt;
use crate::DbScan;

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
    Root::new(
        db,
        file_path(path),
        RootKind::Library,
        vec![],
        vec![],
        vec![],
    )
}

/// Stash `pkg` in `root.packages` and register `root` on
/// `library_roots`, mirroring what the library scanner does after
/// `set_package` returns. The identity map finds the package without this
/// step. It matters for visibility to analysis and for `root_by_package()`.
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
        files,
        Vec::new(),
    );
    register_package(&mut db, long, p2);
    assert_eq!(p1, p2);

    let file = p1.files(&db)[0];
    assert_eq!(file.root(&db), Some(long));
    assert_eq!(all_known_files(&db), &vec![file]);
}

#[test]
fn test_upsert_re_promotes_editor_owned_file_from_orphan() {
    // An editor-open file in `OrphanRoot` should move into `pkg.files` when a
    // scan of the package's root lists it. Same `File` entity, editor
    // contents preserved (the scan's disk snapshot doesn't overwrite),
    // and the orphan reference is cleaned up so the orphan-placement
    // invariant (`package == None`) stays honest.

    let mut db = OakDatabase::new();

    // Editor opens the file before any scan -> orphan.
    let r_path = file_path("/lib/pkg/R/a.R");
    let file = db.upsert_editor(r_path.clone(), "editor content".to_string());
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);

    // Now a library scan picks up the same URL as part of a package.
    let lib = empty_library_root(&db, "/lib");
    let pkg = lib.set_package(
        &mut db,
        file_path("/lib/pkg/DESCRIPTION"),
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
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
fn test_set_package_after_eviction_changes_owning_root() {
    // Package identity is independent of root identity. Re-adding its path
    // under a fresh `Root` must make `root_by_package()` resolve to that root.
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
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, old, p1);
    assert_eq!(db.root_by_package(p1), Some(old));

    old.evict(&mut db);
    db.library_roots().set_roots(&mut db).to(vec![]);
    assert_eq!(db.root_by_package(p1), None);

    let p2 = new.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, new, p2);
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(new));
}

#[test]
fn test_set_library_paths_eviction_keeps_open_buffer_visible() {
    // Removing a library path must keep open package sources visible as
    // standalone files in `OrphanRoot`, even when the sources live elsewhere.
    let lib = tempfile::tempdir().unwrap();
    write_package(&lib.path().join("pkg"), "pkg", &[]);
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[lib.path().to_path_buf()]);
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let path = FilePath::from_path_buf(sources.path().join("a.R")).unwrap();
    let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
    assert_eq!(pkg.files(&db), &[file]);

    db.set_library_paths(&[]);

    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
    assert_eq!(file.source_text(&db), "edited <- 1\n");
}

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
