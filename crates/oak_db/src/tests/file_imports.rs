use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::ImportLayer;
use crate::Package;

/// Create a library root containing one installed package named `name`.
/// Returns the package and the root, both already wired up: the root's
/// `packages` is set to `[pkg]`. Callers register the returned roots on
/// `LibraryRoots` to make `package_by_name` see them.
fn make_installed(db: &mut TestDb, name: &str) -> (crate::Root, Package) {
    let root = library_root(db, &format!("libs/{name}"));
    let pkg = Package::new(
        db,
        root,
        name.to_string(),
        Some("1.0.0".to_string()),
        Namespace::default(),
        Vec::new(),
        None,
    );
    root.set_packages(db).to(vec![pkg]);
    (root, pkg)
}

/// Register a set of installed packages on `LibraryRoots`. Replaces any
/// previously registered library roots.
fn install_packages(db: &mut TestDb, names: &[&str]) -> Vec<Package> {
    let mut roots = Vec::new();
    let mut packages = Vec::new();
    for &name in names {
        let (root, pkg) = make_installed(db, name);
        roots.push(root);
        packages.push(pkg);
    }
    db.library_roots().set_roots(db).to(roots);
    packages
}

#[test]
fn test_script_with_no_attaches_returns_only_default_search_path() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["base", "stats"]);
    let base = packages[0];
    let stats = packages[1];

    let file = File::new(&db, file_url("a.R"), "x <- 1\n".to_string(), None);
    let layers = file.imports(&db);

    // Only `stats` and `base` are registered in this test; the other
    // default-search-path packages are absent and drop out.
    let packages: Vec<Package> = layers
        .iter()
        .map(|layer| match layer {
            ImportLayer::PackageExports(p) => *p,
            other => panic!("unexpected layer: {other:?}"),
        })
        .collect();
    assert_eq!(packages, vec![stats, base]);
}

#[test]
fn test_script_attach_produces_package_exports_layer_in_source_order() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["dplyr", "ggplot2"]);
    let dplyr = packages[0];
    let ggplot2 = packages[1];

    let file = File::new(
        &db,
        file_url("a.R"),
        "library(dplyr)\nlibrary(ggplot2)\n".to_string(),
        None,
    );
    let layers = file.imports(&db);

    let attached: Vec<Package> = layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::PackageExports(p) => Some(*p),
            _ => None,
        })
        .collect();

    // dplyr and ggplot2 appear first (in source order), then the
    // default search path (empty in this setup, no base etc).
    assert_eq!(attached, vec![dplyr, ggplot2]);
}

#[test]
fn test_script_attach_to_unregistered_package_drops_layer() {
    let db = TestDb::new();
    // No `dplyr` in any library root.
    let file = File::new(&db, file_url("a.R"), "library(dplyr)\n".to_string(), None);

    let layers = file.imports(&db);
    assert!(layers.is_empty());
}

#[test]
fn test_package_file_emits_namespace_and_collation_layers() {
    let mut db = TestDb::new();
    let installed = install_packages(&mut db, &["rlang", "base"]);
    let rlang = installed[0];

    let namespace = Namespace {
        imports: vec![Import {
            name: "abort".to_string(),
            package: "rlang".to_string(),
        }],
        package_imports: vec!["rlang".to_string()],
        ..Default::default()
    };

    // Build a workspace package with two R files. `Package.files`
    // holds them in declaration order; `package_layers` walks that
    // order to emit `File` layers.
    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        workspace,
        "pkg".to_string(),
        None,
        namespace,
        Vec::new(),
        None,
    );
    let first = File::new(
        &db,
        file_url("w/pkg/R/_a.R"),
        "first <- 1\n".to_string(),
        Some(pkg),
    );
    let second = File::new(
        &db,
        file_url("w/pkg/R/b.R"),
        "second <- 2\n".to_string(),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![first, second]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let layers = second.imports(&db);

    let mut shape = Vec::new();
    for layer in layers {
        match layer {
            ImportLayer::PackageImports(map) => {
                let mut entries: Vec<(String, String)> =
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                entries.sort();
                shape.push(format!("PackageImports({entries:?})"));
            },
            ImportLayer::PackageExports(p) => {
                shape.push(format!("PackageExports({})", p.name(&db)));
            },
            ImportLayer::File(f) => {
                shape.push(format!(
                    "File({})",
                    f.url(&db).as_url().path().rsplit('/').next().unwrap_or("?")
                ));
            },
        }
    }

    let _ = rlang;
    assert_eq!(shape, vec![
        "PackageImports([(\"abort\", \"rlang\")])".to_string(),
        "PackageExports(rlang)".to_string(),
        "File(_a.R)".to_string(),
        "File(b.R)".to_string(),
        "PackageExports(base)".to_string(),
    ]);
}

#[test]
fn test_imports_is_cached_per_file() {
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["dplyr"]);

    let file = File::new(&db, file_url("a.R"), "library(dplyr)\n".to_string(), None);
    let _ = file.imports(&db);
    let _ = file.imports(&db);

    assert_eq!(db.executions("imports"), 1);
}
