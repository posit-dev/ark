use biome_rowan::TextSize;
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

fn make_file(db: &mut TestDb, path: &str, contents: &str) -> File {
    intern_file(db, file_url(path), contents.to_string(), None)
}

fn make_package_file(db: &mut TestDb, path: &str, contents: &str, package: Package) -> File {
    intern_file(
        db,
        file_url(path),
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
        None,
    )
}

fn workspace_package(db: &TestDb, name: &str, collation: Option<Vec<String>>) -> Package {
    Package::new(
        db,
        name.to_string(),
        PackageOrigin::Workspace {
            root: workspace_root(db, &format!("workspace/{name}")),
        },
        Namespace::default(),
        collation,
    )
}

fn attached_packages(layers: &[ImportLayer]) -> Vec<Package> {
    layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::PackageExports(p) => Some(*p),
            _ => None,
        })
        .collect()
}

fn collation_files(layers: &[ImportLayer]) -> Vec<File> {
    layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::File(f) => Some(*f),
            _ => None,
        })
        .collect()
}

#[test]
fn script_cursor_before_any_attach_sees_no_attached_packages() {
    let mut db = TestDb::new();
    let dplyr = installed_package(&db, "dplyr");
    let ggplot2 = installed_package(&db, "ggplot2");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![dplyr, ggplot2]);

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let layers = file.imports_at(&db, TextSize::from(0));
    assert_eq!(attached_packages(&layers), Vec::<Package>::new());
}

#[test]
fn script_cursor_after_all_attaches_sees_all_in_lifo_order() {
    let mut db = TestDb::new();
    let dplyr = installed_package(&db, "dplyr");
    let ggplot2 = installed_package(&db, "ggplot2");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![dplyr, ggplot2]);

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.len() as u32);
    let layers = file.imports_at(&db, offset);
    // LIFO: latest `library()` call comes first.
    assert_eq!(attached_packages(&layers), vec![ggplot2, dplyr]);
}

#[test]
fn script_cursor_between_attaches_sees_only_earlier_ones() {
    let mut db = TestDb::new();
    let dplyr = installed_package(&db, "dplyr");
    let ggplot2 = installed_package(&db, "ggplot2");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![dplyr, ggplot2]);

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.find("library(ggplot2)").unwrap() as u32);
    let layers = file.imports_at(&db, offset);
    assert_eq!(attached_packages(&layers), vec![dplyr]);
}

#[test]
fn function_body_sees_file_scope_attaches_even_if_after_function_in_source() {
    // R's runtime. File-scope `library()` calls run before any function
    // body executes, so the function sees the package regardless of source
    // order. The offset filter must override its "before cursor" rule for
    // file-scope attaches when the cursor is inside a function body.
    let mut db = TestDb::new();
    let dplyr = installed_package(&db, "dplyr");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![dplyr]);

    let source = "f <- function() {\n  x\n}\nlibrary(dplyr)\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.find("x\n}").unwrap() as u32);
    let layers = file.imports_at(&db, offset);
    assert!(attached_packages(&layers).contains(&dplyr));
}

#[test]
fn package_top_level_sees_predecessor_collation_only() {
    let mut db = TestDb::new();
    let base = installed_package(&db, "base");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![base]);

    let pkg = workspace_package(
        &db,
        "pkg",
        Some(vec![
            "a.R".to_string(),
            "b.R".to_string(),
            "c.R".to_string(),
        ]),
    );
    db.source_graph()
        .set_workspace_packages(&mut db)
        .to(vec![pkg]);
    let a = make_package_file(&mut db, "/workspace/pkg/R/a.R", "first <- 1\n", pkg);
    let b_source = "x <- 1\n";
    let b = make_package_file(&mut db, "/workspace/pkg/R/b.R", b_source, pkg);
    let _c = make_package_file(&mut db, "/workspace/pkg/R/c.R", "second <- 2\n", pkg);

    // Cursor at top-level in b. Only a (the predecessor) is visible.
    let offset = TextSize::from(b_source.find('x').unwrap() as u32);
    let layers = b.imports_at(&db, offset);
    assert_eq!(collation_files(&layers), vec![a]);
}

#[test]
fn package_function_body_sees_other_collation_files_in_lifo_order() {
    let mut db = TestDb::new();
    let base = installed_package(&db, "base");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![base]);

    let pkg = workspace_package(
        &db,
        "pkg",
        Some(vec![
            "a.R".to_string(),
            "b.R".to_string(),
            "c.R".to_string(),
        ]),
    );
    db.source_graph()
        .set_workspace_packages(&mut db)
        .to(vec![pkg]);
    let a = make_package_file(&mut db, "/workspace/pkg/R/a.R", "first <- 1\n", pkg);
    let b_source = "f <- function() {\n  x\n}\n";
    let b = make_package_file(&mut db, "/workspace/pkg/R/b.R", b_source, pkg);
    let c = make_package_file(&mut db, "/workspace/pkg/R/c.R", "second <- 2\n", pkg);

    // Cursor inside f's body. Full lazy view (same as `imports()`).
    // Other collation files appear in LIFO order. Self (b) is excluded
    // since its own top-level bindings live in `exports`.
    let offset = TextSize::from(b_source.find("x\n}").unwrap() as u32);
    let layers = b.imports_at(&db, offset);
    assert_eq!(collation_files(&layers), vec![c, a]);
}

#[test]
fn package_top_level_predecessors_appear_in_lifo_order() {
    // Multiple predecessors of the cursor's file appear latest-first
    // (LIFO), matching R's namespace where the most recently sourced
    // collation file's bindings win.
    let mut db = TestDb::new();
    let base = installed_package(&db, "base");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![base]);

    let pkg = workspace_package(
        &db,
        "pkg",
        Some(vec![
            "a.R".to_string(),
            "b.R".to_string(),
            "c.R".to_string(),
        ]),
    );
    db.source_graph()
        .set_workspace_packages(&mut db)
        .to(vec![pkg]);
    let a = make_package_file(&mut db, "/workspace/pkg/R/a.R", "first <- 1\n", pkg);
    let b = make_package_file(&mut db, "/workspace/pkg/R/b.R", "second <- 2\n", pkg);
    let c_source = "x <- 1\n";
    let c = make_package_file(&mut db, "/workspace/pkg/R/c.R", c_source, pkg);

    let offset = TextSize::from(c_source.find('x').unwrap() as u32);
    let layers = c.imports_at(&db, offset);
    // Predecessors of c are [a, b] in collation order. LIFO gives [b, a].
    assert_eq!(collation_files(&layers), vec![b, a]);
}

#[test]
fn package_namespace_and_base_layers_always_visible() {
    let mut db = TestDb::new();
    let base = installed_package(&db, "base");
    db.source_graph()
        .set_installed_packages(&mut db)
        .to(vec![base]);

    let pkg = workspace_package(&db, "pkg", Some(vec!["a.R".to_string()]));
    db.source_graph()
        .set_workspace_packages(&mut db)
        .to(vec![pkg]);
    let file = make_package_file(&mut db, "/workspace/pkg/R/a.R", "x <- 1\n", pkg);

    let layers = file.imports_at(&db, TextSize::from(0));
    assert!(attached_packages(&layers).contains(&base));
}
