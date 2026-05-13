use oak_package_metadata::namespace::Namespace;
use url::Url;

use crate::collation_files;
use crate::tests::test_db::TestDb;
use crate::vfs_scan;
use crate::vfs_scan::FileDescriptor;
use crate::vfs_scan::PackageDescriptor;
use crate::vfs_scan::ScanResult;
use crate::Db;
use crate::Package;
use crate::SourceNode;
use crate::Vfs;

fn url(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).unwrap()
}

fn parent_package(db: &TestDb, file: crate::File) -> Option<Package> {
    match file.parent(db)? {
        SourceNode::Package(p) => Some(p),
        SourceNode::Script(_) => None,
    }
}

fn collation_basenames(db: &TestDb, pkg: Package) -> Vec<String> {
    collation_files(db, pkg)
        .iter()
        .filter_map(|f| {
            f.url(db)
                .as_url()
                .path()
                .rsplit('/')
                .next()
                .map(|s| s.to_string())
        })
        .collect()
}

fn package_descriptor(
    root: std::path::PathBuf,
    name: &str,
    collation_spec: Option<Vec<String>>,
    files: &[(&str, &str)],
) -> PackageDescriptor {
    let r_dir = root.join("R");
    PackageDescriptor {
        root,
        name: name.to_string(),
        namespace: Namespace::default(),
        collation_spec,
        files: files
            .iter()
            .map(|(basename, contents)| FileDescriptor {
                path: r_dir.join(basename),
                contents: (*contents).to_string(),
            })
            .collect(),
    }
}

#[test]
fn update_file_creates_script_entry() {
    let mut db = TestDb::new();
    let mut vfs = Vfs::new();

    let file = vfs.update_file(&mut db, url("/a.R"), "x <- 1\n".to_string());

    assert_eq!(vfs.url_to_file(&db, &url("/a.R")), Some(file));
    assert_eq!(file.contents(&db), "x <- 1\n");
    assert_eq!(parent_package(&db, file), None);
    assert_eq!(db.source_graph().scripts(&db).len(), 1);
}

#[test]
fn update_file_twice_reuses_file_entity() {
    let mut db = TestDb::new();
    let mut vfs = Vfs::new();

    let f1 = vfs.update_file(&mut db, url("/a.R"), "x <- 1\n".to_string());
    let f2 = vfs.update_file(&mut db, url("/a.R"), "x <- 2\n".to_string());

    assert_eq!(f1, f2);
    assert_eq!(f1.contents(&db), "x <- 2\n");
    assert_eq!(db.source_graph().scripts(&db).len(), 1);
}

#[test]
fn remove_file_drops_script_entry() {
    let mut db = TestDb::new();
    let mut vfs = Vfs::new();

    vfs.update_file(&mut db, url("/a.R"), "x <- 1\n".to_string());
    vfs.remove_file(&mut db, &url("/a.R"));

    assert_eq!(vfs.url_to_file(&db, &url("/a.R")), None);
    assert!(db.source_graph().scripts(&db).is_empty());
}

#[test]
fn remove_file_is_noop_for_unknown_url() {
    let mut db = TestDb::new();
    let mut vfs = Vfs::new();
    vfs.remove_file(&mut db, &url("/never.R"));
    assert!(db.source_graph().scripts(&db).is_empty());
}

#[test]
fn rename_file_preserves_contents() {
    let mut db = TestDb::new();
    let mut vfs = Vfs::new();

    vfs.update_file(&mut db, url("/old.R"), "x <- 1\n".to_string());
    vfs.rename_file(&mut db, &url("/old.R"), url("/new.R"));

    assert_eq!(vfs.url_to_file(&db, &url("/old.R")), None);
    let renamed = vfs.url_to_file(&db, &url("/new.R")).unwrap();
    assert_eq!(renamed.contents(&db), "x <- 1\n");
    assert_eq!(db.source_graph().scripts(&db).len(), 1);
}

#[test]
fn set_workspace_roots_writes_to_input() {
    let mut db = TestDb::new();
    let mut vfs = Vfs::new();

    vfs.set_workspace_roots(&mut db, vec![url("/proj"), url("/other")]);

    assert_eq!(db.workspace_roots().roots(&db).len(), 2);
}

#[test]
fn apply_scan_creates_workspace_package() {
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");

    let scan = ScanResult {
        packages: vec![package_descriptor(pkg_root.clone(), "mypkg", None, &[(
            "foo.R",
            "foo <- 1\n",
        )])],
        scripts: Vec::new(),
    };

    let mut db = TestDb::new();
    let mut vfs = Vfs::new();
    vfs.apply_scan(&mut db, scan);

    let packages = db.source_graph().workspace_packages(&db);
    assert_eq!(packages.len(), 1);
    let pkg = packages[0];
    assert_eq!(pkg.name(&db), "mypkg");

    let files = collation_files(&db, pkg);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].contents(&db), "foo <- 1\n");
    assert_eq!(parent_package(&db, files[0]), Some(pkg));
}

#[test]
fn apply_scan_creates_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let scan = ScanResult {
        packages: Vec::new(),
        scripts: vec![FileDescriptor {
            path: dir.path().join("a.R"),
            contents: "x <- 1\n".to_string(),
        }],
    };

    let mut db = TestDb::new();
    let mut vfs = Vfs::new();
    vfs.apply_scan(&mut db, scan);

    assert_eq!(db.source_graph().scripts(&db).len(), 1);
}

#[test]
fn update_file_attaches_to_known_package_with_alphabetical_collation() {
    // Default-alphabetical collation. A new file added through
    // `update_file` shows up in `collation_files` at its alphabetical
    // position because option-3 derives the list on read.
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");

    let scan = ScanResult {
        packages: vec![package_descriptor(pkg_root.clone(), "mypkg", None, &[
            ("bbb.R", "b\n"),
            ("zzz.R", "z\n"),
        ])],
        scripts: Vec::new(),
    };

    let mut db = TestDb::new();
    let mut vfs = Vfs::new();
    vfs.apply_scan(&mut db, scan);

    let aaa_url = Url::from_file_path(pkg_root.join("R/aaa.R")).unwrap();
    let new_file = vfs.update_file(&mut db, aaa_url, "a\n".to_string());

    let pkg = db.source_graph().workspace_packages(&db)[0];
    assert_eq!(parent_package(&db, new_file), Some(pkg));
    assert_eq!(collation_basenames(&db, pkg), vec![
        "aaa.R", "bbb.R", "zzz.R"
    ]);
}

#[test]
fn update_file_with_explicit_collate_spec_omits_unknown_basename() {
    // Explicit `Collate` field pins the order in `Package.collation`.
    // A new file added at runtime isn't in the spec, so
    // `collation_files` skips it. Editing `DESCRIPTION` is the only
    // way to land a basename in the collation.
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");

    let scan = ScanResult {
        packages: vec![package_descriptor(
            pkg_root.clone(),
            "mypkg",
            Some(vec!["zzz.R".to_string(), "aaa.R".to_string()]),
            &[("zzz.R", "z\n"), ("aaa.R", "a\n")],
        )],
        scripts: Vec::new(),
    };

    let mut db = TestDb::new();
    let mut vfs = Vfs::new();
    vfs.apply_scan(&mut db, scan);

    let mmm_url = Url::from_file_path(pkg_root.join("R/mmm.R")).unwrap();
    vfs.update_file(&mut db, mmm_url, "m\n".to_string());

    let pkg = db.source_graph().workspace_packages(&db)[0];
    assert_eq!(collation_basenames(&db, pkg), vec!["zzz.R", "aaa.R"]);
}

#[test]
fn remove_then_readd_preserves_explicit_collate_position() {
    // Option-3's win. With `Collate` set, removing an `R/` file then
    // adding it back keeps the file at its declared spec position.
    // The spec never moves through the cycle.
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");

    let scan = ScanResult {
        packages: vec![package_descriptor(
            pkg_root.clone(),
            "mypkg",
            Some(vec![
                "middle.R".to_string(),
                "first.R".to_string(),
                "last.R".to_string(),
            ]),
            &[("middle.R", "m\n"), ("first.R", "f\n"), ("last.R", "l\n")],
        )],
        scripts: Vec::new(),
    };

    let mut db = TestDb::new();
    let mut vfs = Vfs::new();
    vfs.apply_scan(&mut db, scan);

    let middle_url = Url::from_file_path(pkg_root.join("R/middle.R")).unwrap();
    vfs.remove_file(&mut db, &middle_url);
    vfs.update_file(&mut db, middle_url, "m\n".to_string());

    let pkg = db.source_graph().workspace_packages(&db)[0];
    assert_eq!(collation_basenames(&db, pkg), vec![
        "middle.R", "first.R", "last.R"
    ]);
}

#[test]
fn scan_then_apply_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let pkg_root = dir.path().join("mypkg");
    std::fs::create_dir_all(pkg_root.join("R")).unwrap();
    std::fs::write(
        pkg_root.join("DESCRIPTION"),
        "Package: mypkg\nVersion: 0.0.1\n",
    )
    .unwrap();
    std::fs::write(pkg_root.join("R/foo.R"), "foo <- 1\n").unwrap();
    std::fs::write(dir.path().join("script.R"), "x <- 1\n").unwrap();

    let scan = vfs_scan::scan(&[dir.path().to_path_buf()]);

    let mut db = TestDb::new();
    let mut vfs = Vfs::new();
    vfs.apply_scan(&mut db, scan);

    assert_eq!(db.source_graph().workspace_packages(&db).len(), 1);
    assert_eq!(db.source_graph().scripts(&db).len(), 1);
    let pkg = db.source_graph().workspace_packages(&db)[0];
    let files = collation_files(&db, pkg);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].contents(&db), "foo <- 1\n");
}
