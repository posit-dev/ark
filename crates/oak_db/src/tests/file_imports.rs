use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::File;
use crate::ImportLayer;
use crate::Package;
use crate::PackageOrigin;
use crate::SourceNode;

fn make_file(db: &mut TestDb, name: &str, contents: &str) -> File {
    intern_file(db, file_url(name), contents.to_string(), None)
}

fn make_package_file(db: &mut TestDb, name: &str, contents: &str, package: Package) -> File {
    intern_file(
        db,
        file_url(name),
        contents.to_string(),
        Some(SourceNode::Package(package)),
    )
}

fn installed_package(db: &TestDb, name: &str) -> Package {
    Package::new(
        db,
        name.to_string(),
        PackageOrigin::Installed {
            version: "1.0.0".to_string(),
            libpath: file_url(&format!("libs/{name}")),
        },
        Namespace::default(),
        Vec::new(),
    )
}

fn workspace_package(
    db: &TestDb,
    name: &str,
    namespace: Namespace,
    collation: Vec<File>,
) -> Package {
    Package::new(
        db,
        name.to_string(),
        PackageOrigin::Workspace {
            root: workspace_root(db, &format!("workspace/{name}")),
        },
        namespace,
        collation,
    )
}

fn register_installed(db: &mut TestDb, packages: Vec<Package>) {
    db.source_graph().set_installed_packages(db).to(packages);
}

fn register_workspace(db: &mut TestDb, packages: Vec<Package>) {
    db.source_graph().set_workspace_packages(db).to(packages);
}

#[test]
fn script_with_no_attaches_returns_only_default_search_path() {
    let mut db = TestDb::new();
    let base = installed_package(&db, "base");
    let stats = installed_package(&db, "stats");
    register_installed(&mut db, vec![base, stats]);

    let file = make_file(&mut db, "a.R", "x <- 1\n");
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
fn script_attach_produces_package_exports_layer_in_lifo_order() {
    let mut db = TestDb::new();
    let dplyr = installed_package(&db, "dplyr");
    let ggplot2 = installed_package(&db, "ggplot2");
    register_installed(&mut db, vec![dplyr, ggplot2]);

    let file = make_file(&mut db, "a.R", "library(dplyr)\nlibrary(ggplot2)\n");
    let layers = file.imports(&db);

    let attached: Vec<Package> = layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::PackageExports(p) => Some(*p),
            _ => None,
        })
        .collect();

    // LIFO: latest `library()` call comes first (matching R's runtime
    // search order). Then the default search path (empty in this setup).
    assert_eq!(attached, vec![ggplot2, dplyr]);
}

#[test]
fn script_attach_to_unregistered_package_drops_layer() {
    let mut db = TestDb::new();
    // No `dplyr` in the source graph.
    let file = make_file(&mut db, "a.R", "library(dplyr)\n");

    let layers = file.imports(&db);
    assert!(layers.is_empty());
}

#[test]
fn package_file_emits_namespace_and_collation_layers() {
    let mut db = TestDb::new();
    let rlang = installed_package(&db, "rlang");
    let base = installed_package(&db, "base");
    register_installed(&mut db, vec![rlang, base]);

    let namespace = Namespace {
        imports: vec![Import {
            name: "abort".to_string(),
            package: "rlang".to_string(),
        }],
        package_imports: vec!["rlang".to_string()],
        ..Default::default()
    };

    // Build the package with two collation files. The second is the one
    // we query, so the first appears as a predecessor.
    let pkg = workspace_package(&db, "pkg", namespace, vec![]);
    register_workspace(&mut db, vec![pkg]);
    let first = make_package_file(&mut db, "/w/pkg/R/_a.R", "first <- 1\n", pkg);
    let second = make_package_file(&mut db, "/w/pkg/R/b.R", "second <- 2\n", pkg);
    pkg.set_collation(&mut db).to(vec![first, second]);

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

    assert_eq!(shape, vec![
        "PackageImports([(\"abort\", \"rlang\")])".to_string(),
        "PackageExports(rlang)".to_string(),
        // Other collation files in reverse declaration order (LIFO).
        // Self (b.R) is excluded. A file's own top-level bindings live
        // in `exports`, not `imports`.
        "File(_a.R)".to_string(),
        "PackageExports(base)".to_string(),
    ]);
}

#[test]
fn imports_is_cached_per_file() {
    let mut db = TestDb::new();
    let dplyr = installed_package(&db, "dplyr");
    register_installed(&mut db, vec![dplyr]);

    let file = make_file(&mut db, "a.R", "library(dplyr)\n");
    let _ = file.imports(&db);
    let _ = file.imports(&db);

    assert_eq!(db.executions("imports"), 1);
}
