use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::RootKind;
use oak_scan::DbExt;

fn write_package(dir: &Path, name: &str, r_files: &[(&str, &str)]) {
    fs::create_dir_all(dir.join("R")).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 0.0.0\n"),
    )
    .unwrap();
    for (basename, contents) in r_files {
        fs::write(dir.join("R").join(basename), contents).unwrap();
    }
}

#[test]
fn test_scan_empty_workspace_registers_empty_root() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let roots = db.workspace_roots().roots(&db).clone();
    assert_eq!(roots.len(), 1);
    let root = roots[0];
    assert_eq!(root.kind(&db), RootKind::Workspace);
    assert!(root.packages(&db).is_empty());
    assert!(root.scripts(&db).is_empty());
}

#[test]
fn test_scan_workspace_discovers_package_at_root() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(tmp.path(), "myproj", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let packages = db.workspace_roots().roots(&db)[0].packages(&db).clone();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name(&db), "myproj");
}

#[test]
fn test_scan_workspace_discovers_multiple_nested_packages() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg1"), "pkg1", &[("a.R", "x <- 1\n")]);
    write_package(&tmp.path().join("pkg2"), "pkg2", &[("b.R", "y <- 2\n")]);
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let packages = db.workspace_roots().roots(&db)[0].packages(&db).clone();
    assert_eq!(packages.len(), 2);
    let mut names: Vec<&str> = packages.iter().map(|p| p.name(&db).as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["pkg1", "pkg2"]);
}

#[test]
fn test_scan_workspace_collects_top_level_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("analysis.R"), "x <- 1\n").unwrap();
    fs::write(tmp.path().join("helpers.R"), "y <- 2\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let scripts = db.workspace_roots().roots(&db)[0].scripts(&db).clone();
    assert_eq!(scripts.len(), 2);
    let mut basenames: Vec<String> = scripts
        .iter()
        .map(|f| {
            f.url(&db)
                .as_url()
                .path()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    basenames.sort();
    assert_eq!(basenames, vec!["analysis.R", "helpers.R"]);
}

#[test]
fn test_scan_workspace_excludes_package_r_files_from_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("inside.R", "z <- 3\n")]);
    fs::write(tmp.path().join("outside.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    let scripts = root.scripts(&db).clone();
    let packages = root.packages(&db).clone();

    assert_eq!(scripts.len(), 1);
    assert!(scripts[0].url(&db).as_url().path().ends_with("outside.R"));
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].files(&db).len(), 1);
}

#[test]
fn test_scan_workspace_excludes_files_in_package_subdirs() {
    // R files in tests/, inst/, etc. are package-internal and shouldn't
    // surface as workspace scripts.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::create_dir_all(tmp.path().join("pkg/tests")).unwrap();
    fs::write(tmp.path().join("pkg/tests/test-foo.R"), "test code\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let scripts = db.workspace_roots().roots(&db)[0].scripts(&db).clone();
    assert!(scripts.is_empty());
}

#[test]
fn test_scan_workspace_honors_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    // Set up as a git-ignored project.
    fs::write(tmp.path().join(".gitignore"), "ignored.R\nbuild/\n").unwrap();
    fs::write(tmp.path().join("ignored.R"), "secret <- 1\n").unwrap();
    fs::write(tmp.path().join("visible.R"), "shown <- 1\n").unwrap();
    fs::create_dir_all(tmp.path().join("build")).unwrap();
    fs::write(tmp.path().join("build/inbuild.R"), "z <- 3\n").unwrap();
    // The `ignore` crate requires a real `.git` directory (or other
    // marker) to apply `.gitignore`. Without one, `.gitignore` files
    // along the walk path are still respected, but only as `.gitignore`
    // not `.git`-anchored.
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let scripts = db.workspace_roots().roots(&db)[0].scripts(&db).clone();
    let basenames: Vec<String> = scripts
        .iter()
        .map(|f| {
            f.url(&db)
                .as_url()
                .path()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(basenames, vec!["visible.R"]);
}

#[test]
fn test_scan_workspace_preserves_orphan_content_on_promotion() {
    // Editor opens a URL before any scan -> orphan File with user-edited
    // contents. Later scan classifies it as a workspace script: the new
    // File entity inherits the orphan's contents, not the disk snapshot.
    let tmp = tempfile::tempdir().unwrap();
    let r_path = tmp.path().join("draft.R");
    fs::write(&r_path, "disk_version <- 1\n").unwrap();
    let mut db = OakDatabase::new();

    // Editor event before any scan.
    let url = UrlId::from_file_path(&r_path).unwrap();
    db.upsert_editor(url.clone(), "edited_version <- 2\n".to_string());

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let file = db
        .file_by_url(&url)
        .expect("script should be findable after scan");
    // The scanner inherited the orphan's edits rather than re-reading disk.
    assert_eq!(file.contents(&db), "edited_version <- 2\n");
    // The orphan reference is dropped when the file is promoted into a
    // workspace container.
    assert!(!db.orphan_root().files(&db).contains(&file));
}

#[test]
fn test_scan_workspace_preserves_package_file_content_on_promotion() {
    // Same content-preservation across the orphan -> package transition.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "disk <- 1\n")]);
    let r_path = tmp.path().join("pkg/R/a.R");
    let mut db = OakDatabase::new();

    let url = UrlId::from_file_path(&r_path).unwrap();
    db.upsert_editor(url.clone(), "edited <- 2\n".to_string());

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let file = db.file_by_url(&url).expect("package file findable");
    assert_eq!(file.contents(&db), "edited <- 2\n");
}

#[test]
fn test_scan_multiple_workspace_paths_preserve_order() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    write_package(&tmp1.path().join("first"), "first", &[]);
    write_package(&tmp2.path().join("second"), "second", &[]);
    let mut db = OakDatabase::new();

    let paths: Vec<PathBuf> = vec![tmp1.path().to_path_buf(), tmp2.path().to_path_buf()];
    db.set_workspace_paths(&paths, &std::collections::HashSet::new());

    let roots = db.workspace_roots().roots(&db).clone();
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].packages(&db)[0].name(&db), "first");
    assert_eq!(roots[1].packages(&db)[0].name(&db), "second");
}

#[test]
fn test_scan_workspace_tolerates_non_package_description() {
    // A file literally named `DESCRIPTION` that isn't a valid R package
    // DESCRIPTION (here: missing the required `Package:` field). The
    // scanner reads it, parsing fails, and the directory is silently
    // not classified as a package.
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("not-a-pkg")).unwrap();
    fs::write(
        tmp.path().join("not-a-pkg/DESCRIPTION"),
        "Title: Some other project\nVersion: 1.0\n",
    )
    .unwrap();
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.packages(&db).is_empty());
}

#[test]
fn test_scan_workspace_dedup_keys_on_description_name_not_folder_name() {
    // Two directories share the same basename `pkg` but their
    // DESCRIPTIONs declare different `Package:` values. Both should be
    // discovered as distinct packages: dedup looks at the DESCRIPTION
    // field, not the directory name (matching R's own loading model).
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("work").join("pkg"), "foo", &[(
        "a.R", "x <- 1\n",
    )]);
    write_package(&tmp.path().join("fork").join("pkg"), "bar", &[(
        "b.R", "y <- 2\n",
    )]);
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let packages = db.workspace_roots().roots(&db)[0].packages(&db).clone();
    assert_eq!(packages.len(), 2);
    let mut names: Vec<&str> = packages.iter().map(|p| p.name(&db).as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["bar", "foo"]);
}

#[test]
fn test_scan_workspace_drops_duplicate_package_names() {
    // Two DESCRIPTION files in the same workspace declare the same
    // `Package:` name. The first one (by sorted directory order) wins,
    // the rest are dropped. Without this dedup, both would collapse
    // onto the same `Package` entity and clobber each other's files.
    let tmp = tempfile::tempdir().unwrap();
    // `aaa-clone` sorts before `bbb-original`, so `aaa-clone` is the
    // first occurrence and should win regardless of fs walk order.
    write_package(&tmp.path().join("aaa-clone"), "pkg", &[(
        "a.R",
        "from_aaa\n",
    )]);
    write_package(&tmp.path().join("bbb-original"), "pkg", &[(
        "b.R",
        "from_bbb\n",
    )]);
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    let packages = root.packages(&db).clone();
    assert_eq!(packages.len(), 1);
    let pkg = packages[0];
    assert_eq!(pkg.name(&db), "pkg");

    let files = pkg.files(&db).clone();
    assert_eq!(files.len(), 1);
    assert!(files[0]
        .url(&db)
        .as_url()
        .path()
        .ends_with("aaa-clone/R/a.R"));
}

#[test]
fn test_scan_workspace_excludes_renv_library() {
    // `renv/library/` snapshots vendored R packages, each with its own
    // DESCRIPTION and R/. The workspace scanner walks through
    // `ignore::WalkBuilder` so these don't surface as workspace packages
    // alongside the user's own code. The mechanism is `.gitignore`; the
    // scenario worth pinning is the renv-shaped layout.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("mypkg"), "mypkg", &[("a.R", "x <- 1\n")]);
    write_package(&tmp.path().join("renv/library/R-4.3/dplyr"), "dplyr", &[(
        "dplyr.R",
        "vendored <- 1\n",
    )]);
    write_package(&tmp.path().join("renv/library/R-4.3/tibble"), "tibble", &[
        ("tibble.R", "vendored <- 1\n"),
    ]);
    fs::write(tmp.path().join(".gitignore"), "renv/library/\n").unwrap();
    // The `ignore` crate's `.gitignore` filter activates only when a
    // `.git` marker is present in the walk (see the comment on
    // `test_scan_workspace_honors_gitignore`). Real renv projects always
    // have one.
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let mut db = OakDatabase::new();

    db.set_workspace_paths(
        &[tmp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let packages = db.workspace_roots().roots(&db)[0].packages(&db).clone();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name(&db), "mypkg");
}

#[test]
fn test_set_workspace_paths_preserves_editor_owned_file_across_churn() {
    // The motivating case for routing editor-owned files to `OrphanRoot`
    // (rather than `StaleRoot`) on eviction: a buffer the user has open
    // stays analysable while its workspace folder is removed, and snaps
    // back into `pkg.files` with the same `File` entity when the folder
    // is re-added.
    use std::collections::HashSet;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_workspace_paths(&[tmp.path().to_path_buf()], &HashSet::new());
    let url = UrlId::from_file_path(tmp.path().join("pkg/R/a.R")).unwrap();
    let file = db.file_by_url(&url).unwrap();
    assert!(file.package(&db).is_some());

    // Editor opens the file; subsequent `set_workspace_paths` calls
    // treat it as editor-owned.
    db.upsert_editor(url.clone(), "edited <- 2\n".to_string());
    let editor_owned: HashSet<UrlId> = [url.clone()].into_iter().collect();

    // Workspace folder removed. File routes to orphan, package goes to stale.
    db.set_workspace_paths(&[], &editor_owned);
    let after_remove = db.file_by_url(&url).unwrap();
    assert_eq!(file, after_remove);
    assert_eq!(after_remove.package(&db), None);
    assert!(db.orphan_root().files(&db).contains(&after_remove));
    assert_eq!(after_remove.contents(&db), "edited <- 2\n");

    // Workspace folder re-added. File snaps back into pkg.files, same
    // entity, editor content preserved (the scan's disk snapshot
    // doesn't overwrite).
    db.set_workspace_paths(&[tmp.path().to_path_buf()], &editor_owned);
    let after_readd = db.file_by_url(&url).unwrap();
    assert_eq!(file, after_readd);
    assert!(after_readd.package(&db).is_some());
    assert_eq!(after_readd.contents(&db), "edited <- 2\n");
    // Orphan reference cleaned up by `upsert_root_file`.
    assert!(!db.orphan_root().files(&db).contains(&after_readd));
}

#[test]
fn test_set_workspace_paths_non_editor_owned_file_goes_to_stale() {
    // The other half of the routing: with no editor-owned set, all files
    // route to stale and disappear from analysis until the folder is
    // re-added.
    use std::collections::HashSet;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_workspace_paths(&[tmp.path().to_path_buf()], &HashSet::new());
    let url = UrlId::from_file_path(tmp.path().join("pkg/R/a.R")).unwrap();
    let file = db.file_by_url(&url).unwrap();

    db.set_workspace_paths(&[], &HashSet::new());
    assert!(db.file_by_url(&url).is_none());
    assert!(db.stale_root().files(&db).contains(&file));

    db.set_workspace_paths(&[tmp.path().to_path_buf()], &HashSet::new());
    let resurrected = db.file_by_url(&url).unwrap();
    assert_eq!(file, resurrected);
}

#[test]
fn test_set_workspace_paths_unchanged_path_preserves_root_and_package_identity() {
    // Repeated calls with the same paths don't churn entities: the existing
    // `Root` is reused (no fs walk), and any `Package` salsa caches stay
    // warm. The watcher is the path for in-folder updates.
    use std::collections::HashSet;

    use salsa::plumbing::AsId;

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();

    db.set_workspace_paths(&[tmp.path().to_path_buf()], &HashSet::new());
    let root_id_before = db.workspace_roots().roots(&db)[0].as_id();
    let pkg_id_before = db.workspace_roots().roots(&db)[0].packages(&db)[0].as_id();

    db.set_workspace_paths(&[tmp.path().to_path_buf()], &HashSet::new());
    let root_id_after = db.workspace_roots().roots(&db)[0].as_id();
    let pkg_id_after = db.workspace_roots().roots(&db)[0].packages(&db)[0].as_id();

    assert_eq!(root_id_before, root_id_after);
    assert_eq!(pkg_id_before, pkg_id_after);
}
