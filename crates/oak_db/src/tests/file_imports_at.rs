use biome_rowan::TextSize;
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

fn make_file(db: &mut TestDb, path: &str, contents: &str) -> File {
    File::new(db, file_url(path), contents.to_string(), None)
}

fn make_package_file(db: &mut TestDb, path: &str, contents: &str, package: Package) -> File {
    File::new(db, file_url(path), contents.to_string(), Some(package))
}

/// Register a set of installed packages on `LibraryRoots`, one library
/// root per package. Returns the packages in input order.
fn install_packages(db: &mut TestDb, names: &[&str]) -> Vec<Package> {
    let mut roots = Vec::new();
    let mut packages = Vec::new();
    for &name in names {
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
        roots.push(root);
        packages.push(pkg);
    }
    db.library_roots().set_roots(db).to(roots);
    packages
}

/// Create a workspace package under a fresh `workspace/{name}` root and
/// register the root on `WorkspaceRoots`. Returns the package.
fn install_workspace_package(db: &mut TestDb, name: &str) -> Package {
    let root = workspace_root(db, &format!("workspace/{name}"));
    let pkg = Package::new(
        db,
        root,
        name.to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    root.set_packages(db).to(vec![pkg]);
    db.workspace_roots().set_roots(db).to(vec![root]);
    pkg
}

fn attached_packages(layers: &[ImportLayer]) -> Vec<Package> {
    layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::Package(p) => Some(*p),
            _ => None,
        })
        .collect()
}

fn package_files(layers: &[ImportLayer]) -> Vec<File> {
    layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::File(f) => Some(*f),
            _ => None,
        })
        .collect()
}

#[test]
fn test_script_cursor_before_any_attach_sees_no_attached_packages() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["dplyr", "ggplot2"]);
    let _ = packages;

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let layers = file.imports_at(&db, TextSize::from(0));
    assert_eq!(attached_packages(&layers), Vec::<Package>::new());
}

#[test]
fn test_script_cursor_after_all_attaches_sees_all_in_lifo_order() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["dplyr", "ggplot2"]);
    let dplyr = packages[0];
    let ggplot2 = packages[1];

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.len() as u32);
    let layers = file.imports_at(&db, offset);
    // LIFO: latest `library()` call comes first.
    assert_eq!(attached_packages(&layers), vec![ggplot2, dplyr]);
}

#[test]
fn test_script_cursor_between_attaches_sees_only_earlier_ones() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["dplyr", "ggplot2"]);
    let dplyr = packages[0];

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.find("library(ggplot2)").unwrap() as u32);
    let layers = file.imports_at(&db, offset);
    assert_eq!(attached_packages(&layers), vec![dplyr]);
}

#[test]
fn test_function_body_sees_file_scope_attaches_even_if_after_function_in_source() {
    // R's runtime. File-scope `library()` calls run before any function
    // body executes, so the function sees the package regardless of source
    // order. The offset filter must override its "before cursor" rule for
    // file-scope attaches when the cursor is inside a function body.
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["dplyr"]);
    let dplyr = packages[0];

    let source = "f <- function() {\n  x\n}\nlibrary(dplyr)\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.find("x\n}").unwrap() as u32);
    let layers = file.imports_at(&db, offset);
    assert!(attached_packages(&layers).contains(&dplyr));
}

#[test]
fn test_package_top_level_sees_predecessor_files_only() {
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["base"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let a = make_package_file(&mut db, "/w/pkg/R/a.R", "first <- 1\n", pkg);
    let b_source = "x <- 1\n";
    let b = make_package_file(&mut db, "/w/pkg/R/b.R", b_source, pkg);
    let c = make_package_file(&mut db, "/w/pkg/R/c.R", "second <- 2\n", pkg);
    pkg.set_files(&mut db).to(vec![a, b, c]);

    // Cursor at top-level in b. Only a (the predecessor in `Package.files`)
    // is visible.
    let offset = TextSize::from(b_source.find('x').unwrap() as u32);
    let layers = b.imports_at(&db, offset);
    assert_eq!(package_files(&layers), vec![a]);
}

#[test]
fn test_package_function_body_sees_other_package_files_in_lifo_order() {
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["base"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let a = make_package_file(&mut db, "/w/pkg/R/a.R", "first <- 1\n", pkg);
    let b_source = "f <- function() {\n  x\n}\n";
    let b = make_package_file(&mut db, "/w/pkg/R/b.R", b_source, pkg);
    let c = make_package_file(&mut db, "/w/pkg/R/c.R", "second <- 2\n", pkg);
    pkg.set_files(&mut db).to(vec![a, b, c]);

    // Cursor inside f's body. Full lazy view (same as `imports()`).
    // Other package files appear in LIFO order. Self (b) is excluded
    // since its own top-level bindings live in `exports`.
    let offset = TextSize::from(b_source.find("x\n}").unwrap() as u32);
    let layers = b.imports_at(&db, offset);
    assert_eq!(package_files(&layers), vec![c, a]);
}

#[test]
fn test_package_top_level_predecessors_appear_in_lifo_order() {
    // Multiple predecessors of the cursor's file appear latest-first
    // (LIFO), matching R's namespace where the most recently sourced
    // file's bindings win.
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["base"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let a = make_package_file(&mut db, "/w/pkg/R/a.R", "first <- 1\n", pkg);
    let b = make_package_file(&mut db, "/w/pkg/R/b.R", "second <- 2\n", pkg);
    let c_source = "x <- 1\n";
    let c = make_package_file(&mut db, "/w/pkg/R/c.R", c_source, pkg);
    pkg.set_files(&mut db).to(vec![a, b, c]);

    let offset = TextSize::from(c_source.find('x').unwrap() as u32);
    let layers = c.imports_at(&db, offset);
    // Predecessors of c are [a, b] in declaration order. LIFO gives [b, a].
    assert_eq!(package_files(&layers), vec![b, a]);
}

#[test]
fn test_package_namespace_and_base_layers_always_visible() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["base"]);
    let base = packages[0];
    let pkg = install_workspace_package(&mut db, "pkg");

    let file = make_package_file(&mut db, "/w/pkg/R/a.R", "x <- 1\n", pkg);
    pkg.set_files(&mut db).to(vec![file]);

    let layers = file.imports_at(&db, TextSize::from(0));
    assert!(attached_packages(&layers).contains(&base));
}
