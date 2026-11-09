//! Environment writers and the merged `LibraryRoots` they maintain.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::Environment;
use oak_db::EnvironmentStatus;
use oak_db::FileRevision;
use oak_db::OakDatabase;
use oak_db::Root;
use oak_db::RootKind;
use oak_db::SourceDb;
use oak_package_metadata::tests::mark_installed;
use salsa::Setter;
use tempfile::TempDir;

use crate::environments::reconcile_environments_to;
use crate::environments::wanted_environment_dirs;
use crate::library::list_library;
use crate::library::InstalledPackageEntry;
use crate::scheduler::drain_scheduler;
use crate::DbScan;
use crate::LibraryListing;
use crate::ScanScheduler;

/// A temporary library holding one installed package named `package`.
fn library(package: &str) -> TempDir {
    let lib = tempfile::tempdir().unwrap();
    let dir = lib.path().join(package);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {package}\nVersion: 1.0.0\n"),
    )
    .unwrap();
    mark_installed(&dir);
    lib
}

fn listing(lib: &TempDir) -> LibraryListing {
    LibraryListing {
        path: lib.path().to_path_buf(),
        packages: list_library(lib.path()),
    }
}

fn file_path(path: &Path) -> FilePath {
    FilePath::from_path_buf(path.to_path_buf()).unwrap()
}

fn env_dir(name: &str) -> FilePath {
    FilePath::from_path_buf(PathBuf::from(format!("/envs/{name}"))).unwrap()
}

/// Reconcile to environments at `/envs/{name}` and return them in order.
fn environments(db: &mut OakDatabase, names: &[&str]) -> Vec<Environment> {
    let wanted: Vec<FilePath> = names.iter().map(|name| env_dir(name)).collect();
    reconcile_environments_to(db, &wanted);
    db.workspace_environments().environments(db).clone()
}

fn library_paths(db: &OakDatabase) -> Vec<PathBuf> {
    db.library_roots()
        .roots(db)
        .iter()
        .map(|root| root.path(db).as_path().unwrap().as_std_path().to_path_buf())
        .collect()
}

fn resolved_roots(db: &OakDatabase, environment: Environment) -> Vec<Root> {
    match environment.status(db) {
        EnvironmentStatus::Resolved(roots) => roots.clone(),
        EnvironmentStatus::Unresolved => panic!("Expected a resolved environment"),
    }
}

#[test]
fn test_merged_library_roots_follow_environment_order_and_share_roots() {
    let lib1 = library("one");
    let shared = library("shared");
    let lib2 = library("two");
    let lib3 = library("three");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a", "b"]);

    // Resolution order differs from environment order.
    db.resolve_environment(db.default_environment(), vec![
        listing(&lib3),
        listing(&lib1),
    ]);
    db.resolve_environment(envs[1], vec![listing(&shared), listing(&lib2)]);
    db.resolve_environment(envs[0], vec![listing(&lib1), listing(&shared)]);

    assert_eq!(library_paths(&db), vec![
        lib1.path().to_path_buf(),
        shared.path().to_path_buf(),
        lib2.path().to_path_buf(),
        lib3.path().to_path_buf(),
    ]);
    let a = resolved_roots(&db, envs[0]);
    let b = resolved_roots(&db, envs[1]);
    let default = resolved_roots(&db, db.default_environment());
    assert_eq!(a[1], b[0]);
    assert_eq!(default[1], a[0]);
    assert_eq!(db.library_roots().roots(&db).clone(), vec![
        a[0], a[1], b[1], default[0]
    ]);
}

#[test]
fn test_failed_environment_contributes_no_libraries() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);

    db.record_environment_failure(envs[0], String::from("timed out"));
    db.set_library_paths(&[lib.path().to_path_buf()]);

    assert_eq!(envs[0].status(&db), &EnvironmentStatus::Unresolved);
    assert_eq!(envs[0].last_error(&db), &Some(String::from("timed out")));
    assert_eq!(library_paths(&db), vec![lib.path().to_path_buf()]);
}

#[test]
fn test_failed_rerun_keeps_last_libraries_until_success_clears_error() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);
    db.resolve_environment(envs[0], vec![listing(&lib)]);
    let roots = resolved_roots(&db, envs[0]);

    db.record_environment_failure(envs[0], String::from("exit status 1"));

    assert_eq!(resolved_roots(&db, envs[0]), roots);
    assert_eq!(
        envs[0].last_error(&db),
        &Some(String::from("exit status 1"))
    );
    assert_eq!(db.library_roots().roots(&db).clone(), roots);

    db.resolve_environment(envs[0], vec![listing(&lib)]);

    assert_eq!(resolved_roots(&db, envs[0]), roots);
    assert_eq!(envs[0].last_error(&db), &None);
}

#[test]
fn test_removing_environment_evicts_libraries_only_it_listed() {
    let own = library("own");
    let shared = library("shared");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a", "b"]);
    db.resolve_environment(envs[0], vec![listing(&own), listing(&shared)]);
    db.resolve_environment(envs[1], vec![listing(&shared)]);
    let a = resolved_roots(&db, envs[0]);

    let added = reconcile_environments_to(&mut db, &[env_dir("b")]);

    assert_eq!(added, Vec::new());
    assert_eq!(library_paths(&db), vec![shared.path().to_path_buf()]);
    assert_eq!(db.library_roots().roots(&db).clone(), vec![a[1]]);
    assert!(a[0].packages(&db).is_empty());
    assert!(db.package_by_name("own").is_none());
    assert!(db.package_by_name("shared").is_some());
    assert_eq!(envs[0].status(&db), &EnvironmentStatus::Unresolved);
}

#[test]
fn test_reconcile_keeps_live_environments_and_adds_unresolved_ones() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    let added = reconcile_environments_to(&mut db, &[env_dir("a")]);
    let a = db.workspace_environments().environments(&db)[0];
    assert_eq!(added, vec![a]);
    db.resolve_environment(a, vec![listing(&lib)]);

    let added = reconcile_environments_to(&mut db, &[env_dir("b"), env_dir("a"), env_dir("b")]);

    let live = db.workspace_environments().environments(&db).clone();
    assert_eq!(live.len(), 2);
    assert_eq!(live[1], a);
    assert_eq!(live[0].dir(&db), &Some(env_dir("b")));
    assert_eq!(live[0].status(&db), &EnvironmentStatus::Unresolved);
    assert_eq!(added, vec![live[0]]);
    assert_eq!(library_paths(&db), vec![lib.path().to_path_buf()]);
    assert_eq!(
        resolved_roots(&db, a),
        db.library_roots().roots(&db).clone()
    );
}

#[test]
fn test_wanted_environment_dirs_follow_root_order_and_count_once() {
    let mut db = OakDatabase::new();
    let root = |db: &OakDatabase, path: &str, dirs: &[&str]| {
        Root::new(
            db,
            env_dir(path),
            RootKind::Workspace,
            Vec::new(),
            Vec::new(),
            dirs.iter().map(|dir| env_dir(dir)).collect(),
        )
    };
    // The outer root is its own environment directory and also lists the
    // inner root, which is an environment directory too.
    let outer = root(&db, "proj", &["proj", "proj/inner", "proj/z"]);
    let inner = root(&db, "proj/inner", &["proj/inner", "proj/inner/c"]);
    let other = root(&db, "other", &[]);
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![outer, inner, other]);

    let wanted = wanted_environment_dirs(&db);

    assert_eq!(wanted, vec![
        env_dir("proj"),
        env_dir("proj/inner"),
        env_dir("proj/z"),
        env_dir("proj/inner/c"),
        env_dir("other"),
    ]);
    let added = db.reconcile_environments();
    assert_eq!(added.len(), 5);
    let dirs: Vec<Option<FilePath>> = db
        .workspace_environments()
        .environments(&db)
        .iter()
        .map(|environment| environment.dir(&db).clone())
        .collect();
    assert_eq!(dirs, wanted.into_iter().map(Some).collect::<Vec<_>>());
}

#[test]
fn test_wanted_environment_dirs_from_scanned_root() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join(".Rprofile"), "").unwrap();
    fs::create_dir_all(tmp.path().join("sub")).unwrap();
    fs::write(tmp.path().join("sub/.Renviron"), "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let requests = scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()]);
    drain_scheduler(&mut db, &mut scheduler, requests);

    assert_eq!(wanted_environment_dirs(&db), vec![
        file_path(tmp.path()),
        file_path(&tmp.path().join("sub")),
    ]);
}

#[test]
fn test_resolving_from_listing_walks_no_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);

    db.resolve_environment(envs[0], vec![LibraryListing {
        path: missing.clone(),
        packages: vec![InstalledPackageEntry {
            name: String::from("ghost"),
            description_path: file_path(&missing.join("ghost/DESCRIPTION")),
            description_revision: FileRevision::zero(),
            namespace_revision: FileRevision::zero(),
            index_revision: FileRevision::zero(),
        }],
    }]);

    let ghost = db.package_by_name("ghost").unwrap();
    assert_eq!(
        db.root_by_package(ghost),
        Some(resolved_roots(&db, envs[0])[0])
    );
}

#[test]
fn test_set_library_paths_leaves_default_resolved() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    let default = db.default_environment();
    assert_eq!(
        default.status(&db),
        &EnvironmentStatus::Resolved(Vec::new())
    );

    db.set_library_paths(&[lib.path().to_path_buf()]);

    assert_eq!(db.default_environment(), default);
    assert_eq!(
        default.status(&db),
        &EnvironmentStatus::Resolved(db.library_roots().roots(&db).clone())
    );
    assert_eq!(default.last_error(&db), &None);
    assert_eq!(library_paths(&db), vec![lib.path().to_path_buf()]);
}

#[test]
fn test_mark_default_unresolved_drops_its_libraries() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = db.library_roots().roots(&db)[0];

    db.mark_default_environment_unresolved();

    assert_eq!(
        db.default_environment().status(&db),
        &EnvironmentStatus::Unresolved
    );
    assert_eq!(library_paths(&db), Vec::<PathBuf>::new());
    assert!(root.packages(&db).is_empty());

    db.mark_default_environment_unresolved();
    assert_eq!(library_paths(&db), Vec::<PathBuf>::new());
}

#[test]
fn test_library_leaving_merged_list_keeps_open_buffer_visible() {
    let lib = library("pkg");
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);
    db.resolve_environment(envs[0], vec![listing(&lib)]);
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let path = file_path(&sources.path().join("a.R"));
    let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
    assert_eq!(pkg.files(&db), &[file]);

    reconcile_environments_to(&mut db, &[]);

    assert!(db.library_roots().roots(&db).is_empty());
    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
    assert_eq!(file.source_text(&db), "edited <- 1\n");
}

#[test]
fn test_unchanged_results_write_nothing() {
    let lib1 = library("one");
    let lib2 = library("two");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);
    db.resolve_environment(envs[0], vec![listing(&lib1), listing(&lib2)]);
    db.record_environment_failure(db.default_environment(), String::from("no R"));
    let before = salsa::plumbing::current_revision(&db);

    db.resolve_environment(envs[0], vec![listing(&lib1), listing(&lib2)]);
    db.record_environment_failure(db.default_environment(), String::from("no R"));
    let added = reconcile_environments_to(&mut db, &[env_dir("a")]);

    assert_eq!(added, Vec::new());
    assert_eq!(library_paths(&db), vec![
        lib1.path().to_path_buf(),
        lib2.path().to_path_buf(),
    ]);
    assert_eq!(salsa::plumbing::current_revision(&db), before);
}

#[test]
fn test_result_for_removed_environment_is_ignored() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);
    reconcile_environments_to(&mut db, &[]);

    db.resolve_environment(envs[0], vec![listing(&lib)]);
    db.record_environment_failure(envs[0], String::from("late"));

    assert_eq!(library_paths(&db), Vec::<PathBuf>::new());
    assert_eq!(envs[0].status(&db), &EnvironmentStatus::Unresolved);
    assert_eq!(envs[0].last_error(&db), &None);
}

#[test]
fn test_rescan_in_flight_for_evicted_library_is_dropped() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);
    db.resolve_environment(envs[0], vec![listing(&lib)]);
    let old = db.library_roots().roots(&db)[0];

    let mut scheduler = ScanScheduler::new();
    let entry = file_path(&lib.path().join("pkg"));
    let mut requests = scheduler.apply_watcher_events(&mut db, vec![entry]);
    assert_eq!(requests.len(), 1);
    let request = requests.remove(0);

    reconcile_environments_to(&mut db, &[]);
    let completed = request.run();
    let requests = scheduler.apply_scan_completed(&mut db, completed);

    assert!(requests.is_empty());
    assert!(!scheduler.has_pending_scans());
    assert!(old.packages(&db).is_empty());
    assert!(db.package_by_name("pkg").is_none());

    let envs = environments(&mut db, &["a"]);
    db.resolve_environment(envs[0], vec![listing(&lib)]);
    let new = db.library_roots().roots(&db)[0];
    assert_ne!(new, old);
    assert!(db.package_by_name("pkg").is_some());
}

#[test]
fn test_rescan_in_flight_for_evicted_library_is_dropped_after_path_returns() {
    let lib = library("pkg");
    let mut db = OakDatabase::new();
    let envs = environments(&mut db, &["a"]);
    db.resolve_environment(envs[0], vec![listing(&lib)]);
    let old = db.library_roots().roots(&db)[0];

    let mut scheduler = ScanScheduler::new();
    let entry = file_path(&lib.path().join("pkg"));
    let mut requests = scheduler.apply_watcher_events(&mut db, vec![entry]);
    assert_eq!(requests.len(), 1);
    let completed = requests.remove(0).run();

    // The returning path's listing includes a package the in-flight rescan
    // didn't see, so applying that rescan to the new root would drop it.
    reconcile_environments_to(&mut db, &[]);
    let other = lib.path().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("DESCRIPTION"),
        "Package: other\nVersion: 1.0.0\n",
    )
    .unwrap();
    mark_installed(&other);
    let envs = environments(&mut db, &["a"]);
    db.resolve_environment(envs[0], vec![listing(&lib)]);
    let new = db.library_roots().roots(&db)[0];
    assert_ne!(new, old);

    let requests = scheduler.apply_scan_completed(&mut db, completed);

    assert!(requests.is_empty());
    assert!(!scheduler.has_pending_scans());
    assert!(old.packages(&db).is_empty());
    let names: Vec<&str> = new
        .packages(&db)
        .iter()
        .map(|package| package.name(&db).as_str())
        .collect();
    assert_eq!(names, vec!["other", "pkg"]);
}

#[test]
fn test_file_lookup_reads_written_environments() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.R"), "1\n").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let requests = scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()]);
    drain_scheduler(&mut db, &mut scheduler, requests);
    let added = db.reconcile_environments();
    let file = db
        .file_by_path(&file_path(&tmp.path().join("a.R")))
        .unwrap();

    assert_eq!(file.environment(&db), added[0]);
    assert_eq!(
        file.environment(&db).status(&db),
        &EnvironmentStatus::Unresolved
    );

    db.resolve_environment(added[0], Vec::new());
    assert_eq!(
        file.environment(&db).status(&db),
        &EnvironmentStatus::Resolved(Vec::new())
    );
}
