use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::test_path::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::make_package;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::BindingKind;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::ImportLayer;
use crate::Package;
use crate::VisibleBinding;
use crate::VisibleNames;

/// Split `source` at its `|` marker into the text and the cursor offset.
fn cursor(source: &str) -> (String, TextSize) {
    let offset = source.find('|').unwrap();
    (source.replacen('|', "", 1), TextSize::from(offset as u32))
}

fn make_file(db: &TestDb, path: &str, contents: &str) -> File {
    File::new(
        db,
        file_path(path),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

/// Visible names at the `|` marker of a standalone script.
fn names_at(db: &TestDb, source: &str) -> Vec<String> {
    let (text, offset) = cursor(source);
    let file = make_file(db, "a.R", &text);
    definition_names(db, &file.visible_names_at(db, offset))
}

/// Names whose first binding is a definition, in entry order.
fn definition_names(db: &TestDb, names: &VisibleNames) -> Vec<String> {
    names
        .entries
        .iter()
        .filter(|entry| matches!(entry.bindings[0], VisibleBinding::Definition { .. }))
        .map(|entry| entry.name.text(db).to_string())
        .collect()
}

/// Names bound by an export of `package`, in entry order.
fn package_export_names(db: &TestDb, names: &VisibleNames, package: Package) -> Vec<String> {
    names
        .entries
        .iter()
        .filter(|entry| {
            entry.bindings.iter().any(|binding| {
                matches!(
                    binding,
                    VisibleBinding::PackageExport { package: exporter, .. } if *exporter == package
                )
            })
        })
        .map(|entry| entry.name.text(db).to_string())
        .collect()
}

/// The kinds of `name`'s bindings, in lookup order.
fn binding_kinds(db: &TestDb, names: &VisibleNames, name: &str) -> Vec<BindingKind> {
    let entry = names
        .entries
        .iter()
        .find(|entry| entry.name.text(db) == name)
        .unwrap();
    entry.bindings.iter().map(VisibleBinding::kind).collect()
}

/// The first formal of each of `name`'s definitions, in lookup order.
fn first_formals(db: &TestDb, names: &VisibleNames, name: &str) -> Vec<String> {
    let entry = names
        .entries
        .iter()
        .find(|entry| entry.name.text(db) == name)
        .unwrap();
    entry
        .bindings
        .iter()
        .map(|binding| {
            let VisibleBinding::Definition { definition, .. } = binding else {
                panic!("Expected a definition");
            };
            definition.formals(db).unwrap()[0].name.clone()
        })
        .collect()
}

fn package_names(db: &TestDb, packages: &[Package]) -> Vec<String> {
    packages
        .iter()
        .map(|package| package.name(db).to_string())
        .collect()
}

fn layer_packages(layers: &[ImportLayer]) -> Vec<Package> {
    layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::Import(package) | ImportLayer::Package(package) => Some(*package),
            _ => None,
        })
        .collect()
}

/// Register library packages, each with its NAMESPACE text and one `R/` file.
fn install_library_packages(db: &mut TestDb, packages: &[(&str, &str, &str)]) -> Vec<Package> {
    let mut roots = Vec::new();
    let mut installed = Vec::new();
    for &(name, namespace, source) in packages {
        let namespace = Namespace::parse(namespace).unwrap();
        let path = format!("ws/{name}/R/{name}.R");
        let (package, _) = make_package(db, name, namespace, &[(&path, source)]);
        let root = library_root(db, &format!("libs/{name}"));
        root.set_packages(db).to(vec![package]);
        roots.push(root);
        installed.push(package);
    }
    db.library_roots().set_roots(db).to(roots);
    installed
}

#[test]
fn test_empty_file_sees_no_names() {
    let db = TestDb::new();
    let file = make_file(&db, "a.R", "");
    let names = file.visible_names_at(&db, TextSize::from(0));
    assert_eq!(names.entries, Vec::new());
    assert_eq!(names.packages, Vec::new());
}

#[test]
fn test_empty_top_level_line_sees_earlier_bindings_only() {
    let db = TestDb::new();
    assert_eq!(names_at(&db, "a <- 1\n|\nb <- 2\n"), vec!["a"]);
    assert_eq!(names_at(&db, "|\na <- 1\n"), Vec::<String>::new());
}

#[test]
fn test_cursor_inside_token() {
    let db = TestDb::new();
    assert_eq!(names_at(&db, "abc <- 1\nab|c\n"), vec!["abc"]);
    // The binding whose name the cursor is in hasn't happened yet.
    assert_eq!(names_at(&db, "ab|c <- 1\n"), Vec::<String>::new());
    assert_eq!(names_at(&db, "f <- function(param) pa|ram\n"), vec![
        "param", "f"
    ]);
}

#[test]
fn test_function_body_sees_its_scopes_bindings_anywhere() {
    // A lazy body sees its own and enclosing bindings regardless of position,
    // innermost scope first. Other functions' bodies stay out.
    let db = TestDb::new();
    let source = "\
a <- 1
f <- function(x) {
  y <- 1
  |
  z <- 2
}
g <- function(w) w
b <- 2
";
    assert_eq!(names_at(&db, source), vec![
        "x", "y", "z", "a", "f", "g", "b"
    ]);
}

#[test]
fn test_eager_local_body_sees_earlier_bindings_only() {
    let db = TestDb::new();
    let source = "\
a <- 1
local({
  x <- 1
  |
  y <- 2
})
b <- 2
";
    assert_eq!(names_at(&db, source), vec!["x", "a"]);
}

#[test]
fn test_unfinished_function_at_end_of_file() {
    let db = TestDb::new();
    assert_eq!(
        names_at(&db, "a <- 1\nf <- function(x) {\n  y <- 1\n  |"),
        vec!["x", "y", "a", "f"]
    );
    assert_eq!(names_at(&db, "a <- 1\nf <- function(x) |"), vec![
        "x", "a", "f"
    ]);
}

#[test]
fn test_end_of_unbraced_body_mid_file() {
    let db = TestDb::new();
    assert_eq!(names_at(&db, "f <- function(x) x|\ng <- 1\n"), vec![
        "x", "f", "g"
    ]);
}

#[test]
fn test_cursor_after_closing_brace() {
    let db = TestDb::new();
    assert_eq!(
        names_at(&db, "f <- function(x) { y <- x }|\ng <- 1\n"),
        vec!["f"]
    );
}

#[test]
fn test_sourced_file_names_after_source_call() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let helpers = make_file(&db, "w/helpers.R", "helper <- function() 1\n");
    let (text, after) = cursor("x <- 1\nsource(\"helpers.R\")\n|");
    let main = make_file(&db, "w/main.R", &text);
    root.set_scripts(&mut db).to(vec![helpers, main]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let names = main.visible_names_at(&db, after);
    assert_eq!(definition_names(&db, &names), vec!["x", "helper"]);
    let VisibleBinding::Definition {
        definition: helper,
        kind,
    } = names.entries[1].bindings[0]
    else {
        panic!("Expected a definition");
    };
    assert_eq!(helper.file(&db), helpers);
    assert_eq!(kind, BindingKind::Function);

    let before = TextSize::from(text.find("source").unwrap() as u32);
    let names = main.visible_names_at(&db, before);
    assert_eq!(definition_names(&db, &names), vec!["x"]);
}

#[test]
fn test_sourced_file_sees_sourcing_file_names_bound_before_the_call() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let main = make_file(&db, "w/main.R", "x <- 1\nsource(\"helpers.R\")\ny <- 2\n");
    let (text, offset) = cursor("h <- 1\n|");
    let helpers = make_file(&db, "w/helpers.R", &text);
    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let names = helpers.visible_names_at(&db, offset);
    assert_eq!(definition_names(&db, &names), vec!["h", "x"]);
}

#[test]
fn test_top_level_redefinition_lists_latest_first() {
    let db = TestDb::new();
    let at = |source: &str| {
        let (text, offset) = cursor(source);
        let file = make_file(&db, "a.R", &text);
        file.visible_names_at(&db, offset)
    };

    let names = at("x <- 1\nx <- function(y) y\n|");
    assert_eq!(binding_kinds(&db, &names, "x"), vec![
        BindingKind::Function,
        BindingKind::Unknown
    ]);

    let names = at("x <- function(y) y\nx <- 1\n|");
    assert_eq!(binding_kinds(&db, &names, "x"), vec![
        BindingKind::Unknown,
        BindingKind::Function
    ]);

    let names = at("f <- function(a) 1\nf <- function(b) 2\n|");
    assert_eq!(first_formals(&db, &names, "f"), vec!["b", "a"]);

    // A later binding hasn't run at an eager position.
    let names = at("f <- function(a) 1\nf <- function(b) 2\n|\nf <- function(c) 3\n");
    assert_eq!(first_formals(&db, &names, "f"), vec!["b", "a"]);
}

#[test]
fn test_redefinition_in_function_body_lists_run_bindings_first() {
    let db = TestDb::new();
    let at = |source: &str| {
        let (text, offset) = cursor(source);
        let file = make_file(&db, "a.R", &text);
        file.visible_names_at(&db, offset)
    };

    // The cursor's body has run up to the cursor, so its bindings before the
    // cursor come first, then its later ones, then the enclosing scope's.
    let names = at("\
f <- function(top) 1
g <- function() {
  f <- function(a) 1
  f <- function(b) 2
  |
  f <- function(c) 3
  f <- function(d) 4
}
");
    assert_eq!(first_formals(&db, &names, "f"), vec![
        "b", "a", "d", "c", "top"
    ]);

    let names = at("\
g <- function() {
  x <- 1
  x|
  x <- function(b) 2
}
");
    assert_eq!(binding_kinds(&db, &names, "x"), vec![
        BindingKind::Unknown,
        BindingKind::Function
    ]);

    // An enclosing body may have run to its end, so its latest binding comes
    // first wherever it is.
    let names = at("\
g <- function() {
  f <- function(a) 1
  h <- function() {
    |
  }
  f <- function(b) 2
}
");
    assert_eq!(first_formals(&db, &names, "f"), vec!["b", "a"]);

    let names = at("\
local({
  f <- function(a) 1
  f <- function(b) 2
  |
  f <- function(c) 3
})
");
    assert_eq!(first_formals(&db, &names, "f"), vec!["b", "a"]);
}

#[test]
fn test_redefinition_through_file_exports_lists_latest_first() {
    // Both arms of `if`/`else` stay bound after the file runs, so `a.R`
    // exports both, with the later one first.
    let mut db = TestDb::new();
    let (text, in_body) = cursor("g <- function() {\n  |\n}\n");
    let (package, files) = make_package(&mut db, "pkg", Namespace::default(), &[
        (
            "ws/pkg/R/a.R",
            "if (cond) f <- function(a) 1 else f <- function(b) 2\n",
        ),
        ("ws/pkg/R/b.R", &text),
    ]);
    let root = workspace_root(&db, "ws/pkg");
    root.set_packages(&mut db).to(vec![package]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let names = files[1].visible_names_at(&db, in_body);
    assert_eq!(first_formals(&db, &names, "f"), vec!["b", "a"]);
}

#[test]
fn test_sourced_bindings_take_the_sourced_files_merged_kind() {
    // Each definition reached through `source()` takes `helpers.R`'s kind for
    // the name, merged across both arms, not its own.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let helpers = make_file(
        &db,
        "w/helpers.R",
        "if (cond) h <- 1 else h <- function() 1\nfun <- function() 1\n",
    );
    let (text, after) = cursor("source(\"helpers.R\")\n|");
    let main = make_file(&db, "w/main.R", &text);
    root.set_scripts(&mut db).to(vec![helpers, main]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let names = main.visible_names_at(&db, after);
    assert_eq!(binding_kinds(&db, &names, "h"), vec![
        BindingKind::Unknown,
        BindingKind::Unknown
    ]);
    assert_eq!(binding_kinds(&db, &names, "fun"), vec![
        BindingKind::Function
    ]);

    let entry = names
        .entries
        .iter()
        .find(|entry| entry.name.text(&db) == "h")
        .unwrap();
    let VisibleBinding::Definition { definition, .. } = entry.bindings[0] else {
        panic!("Expected a definition");
    };
    assert_eq!(definition.file(&db), helpers);
    assert_eq!(definition.binding_kind(&db), BindingKind::Function);
}

#[test]
fn test_package_file_names_follow_collation_lookup_order() {
    // A function body runs after the whole collation has loaded, so a later
    // sibling ranks above this file's own top level, and an earlier one below.
    // At the top level only earlier siblings have loaded.
    let mut db = TestDb::new();
    let (text, in_body) = cursor("own <- 1\nf <- function() {\n  |\n}\n");
    let (package, files) = make_package(&mut db, "pkg", Namespace::default(), &[
        ("ws/pkg/R/a.R", "first <- 1\n"),
        ("ws/pkg/R/b.R", &text),
        ("ws/pkg/R/c.R", "last <- 1\n"),
    ]);
    let root = workspace_root(&db, "ws/pkg");
    root.set_packages(&mut db).to(vec![package]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);
    let b = files[1];

    let names = b.visible_names_at(&db, in_body);
    assert_eq!(definition_names(&db, &names), vec![
        "last", "own", "f", "first"
    ]);

    let top_level = TextSize::from(text.find("f <-").unwrap() as u32);
    let names = b.visible_names_at(&db, top_level);
    assert_eq!(definition_names(&db, &names), vec!["own", "first"]);
}

#[test]
fn test_attached_packages_listed_before_they_contribute_names() {
    let mut db = TestDb::new();
    let packages = install_library_packages(&mut db, &[
        // No NAMESPACE exports and no indexed sources yet.
        ("pending", "", ""),
        (
            "pkgb",
            "export(bfun)\n",
            "bfun <- function() 1\ninternal <- 2\n",
        ),
    ]);
    let (pending, pkgb) = (packages[0], packages[1]);

    let (text, offset) = cursor("library(pending)\nlibrary(pkgb)\n|");
    let file = make_file(&db, "a.R", &text);
    let names = file.visible_names_at(&db, offset);

    assert_eq!(package_names(&db, &names.packages), vec!["pkgb", "pending"]);
    assert_eq!(
        package_export_names(&db, &names, pending),
        Vec::<String>::new()
    );
    assert_eq!(package_export_names(&db, &names, pkgb), vec!["bfun"]);

    // Before the attaches, neither package is consulted.
    let names = file.visible_names_at(&db, TextSize::from(0));
    assert_eq!(names.packages, Vec::new());
}

#[test]
fn test_attached_workspace_package_is_not_listed() {
    // A workspace package's files are already scanned, so nothing about it is
    // pending, even though its names are visible.
    let mut db = TestDb::new();
    let namespace = Namespace::parse("export(wfun)\n").unwrap();
    let (package, _) = make_package(&mut db, "wpkg", namespace, &[(
        "ws/wpkg/R/a.R",
        "wfun <- function() 1\n",
    )]);
    let root = workspace_root(&db, "ws/wpkg");
    root.set_packages(&mut db).to(vec![package]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let (text, offset) = cursor("library(wpkg)\n|");
    let file = make_file(&db, "a.R", &text);
    let names = file.visible_names_at(&db, offset);

    assert_eq!(package_export_names(&db, &names, package), vec!["wfun"]);
    assert_eq!(names.packages, Vec::new());
}

#[test]
fn test_local_binding_kind_differs_from_top_level_namesake() {
    let db = TestDb::new();
    let at = |source: &str| {
        let (text, offset) = cursor(source);
        let file = make_file(&db, "a.R", &text);
        file.visible_names_at(&db, offset)
    };

    let names = at("inner <- function() 1\nouter <- function() {\n  inner <- 1\n  |\n}\n");
    assert_eq!(binding_kinds(&db, &names, "inner"), vec![
        BindingKind::Unknown,
        BindingKind::Function
    ]);

    let names = at("inner <- 1\nouter <- function() {\n  inner <- function() 1\n  |\n}\n");
    assert_eq!(binding_kinds(&db, &names, "inner"), vec![
        BindingKind::Function,
        BindingKind::Unknown
    ]);
}

#[test]
fn test_local_function_kind() {
    let db = TestDb::new();
    let (text, offset) = cursor("outer <- function(x) {\n  helper <- function() 1\n  |\n}\n");
    let file = make_file(&db, "a.R", &text);
    let names = file.visible_names_at(&db, offset);
    assert_eq!(binding_kinds(&db, &names, "helper"), vec![
        BindingKind::Function
    ]);
    assert_eq!(binding_kinds(&db, &names, "x"), vec![BindingKind::Unknown]);
}

#[test]
fn test_package_export_kind() {
    let mut db = TestDb::new();
    let packages = install_library_packages(&mut db, &[(
        "pkg",
        "export(fun, value)\n",
        "fun <- function() 1\nvalue <- 2\n",
    )]);
    let pkg = packages[0];

    let (text, offset) = cursor("library(pkg)\n|");
    let file = make_file(&db, "a.R", &text);
    let names = file.visible_names_at(&db, offset);
    assert_eq!(names.entries[0].bindings, vec![
        VisibleBinding::PackageExport {
            package: pkg,
            kind: BindingKind::Function,
        }
    ]);
    assert_eq!(binding_kinds(&db, &names, "value"), vec![
        BindingKind::Unknown
    ]);
}

#[test]
fn test_reexport_sources_are_listed() {
    // `foo`'s kind comes from `source`, so `source`'s pending sources can
    // change the answer although it's never attached.
    let mut db = TestDb::new();
    let packages = install_library_packages(&mut db, &[
        ("source", "export(foo)\n", ""),
        ("reexport", "importFrom(source, foo)\nexport(foo)\n", ""),
    ]);
    let (source, reexport) = (packages[0], packages[1]);

    let (text, offset) = cursor("library(reexport)\n|");
    let file = make_file(&db, "a.R", &text);
    let names = file.visible_names_at(&db, offset);
    assert_eq!(names.packages, vec![reexport, source]);
    assert_eq!(package_export_names(&db, &names, reexport), vec!["foo"]);
}

#[test]
fn test_attached_package_lazy_data_is_visible() {
    let mut db = TestDb::new();
    let packages = install_library_packages(&mut db, &[("pkg", "", "")]);
    let pkg = packages[0];
    pkg.set_lazy_data(&mut db, vec!["dataset".to_string()]);

    let (text, offset) = cursor("library(pkg)\n|");
    let file = make_file(&db, "a.R", &text);
    let names = file.visible_names_at(&db, offset);
    assert_eq!(package_export_names(&db, &names, pkg), vec!["dataset"]);
}

#[test]
fn test_namespace_import_lazy_data_is_not_visible() {
    // R's imports environment holds `dep`'s exports but not its lazy data,
    // which `dep::dataset` or an attach would reach.
    let mut db = TestDb::new();
    let packages = install_library_packages(&mut db, &[(
        "dep",
        "export(fun)\n",
        "fun <- function() 1\n",
    )]);
    let dep = packages[0];
    dep.set_lazy_data(&mut db, vec!["dataset".to_string(), "fun".to_string()]);

    let namespace = Namespace::parse("import(dep)\n").unwrap();
    let (text, offset) = cursor("f <- function() {\n  |\n}\n");
    let (package, files) = make_package(&mut db, "pkg", namespace, &[("ws/pkg/R/a.R", &text)]);
    let root = workspace_root(&db, "ws/pkg");
    root.set_packages(&mut db).to(vec![package]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let names = files[0].visible_names_at(&db, offset);
    assert_eq!(package_export_names(&db, &names, dep), vec!["fun"]);
    assert_eq!(names.packages, vec![dep]);
}

#[test]
fn test_imports_at_agrees_on_the_cursor_scope() {
    // A library attached after the function is visible from its body only, so
    // both queries must place each cursor in the same scope.
    let mut db = TestDb::new();
    install_library_packages(&mut db, &[("pkg", "", "")]);

    let cases = [
        ("f <- function(x) x|\nlibrary(pkg)\n", true),
        ("f <- function(x) |\nlibrary(pkg)\n", true),
        ("library(pkg)\nf <- function(x) {\n  |", true),
        ("f <- function(x) { x }|\nlibrary(pkg)\n", false),
        ("f <- function(x) x\n|\nlibrary(pkg)\n", false),
    ];
    for (source, in_function) in cases {
        let (text, offset) = cursor(source);
        let file = make_file(&db, "a.R", &text);

        let names = file.visible_names_at(&db, offset);
        let layers = file.imports_at(&db, offset);
        assert_eq!(names.packages, layer_packages(&layers));
        assert_eq!(
            definition_names(&db, &names).contains(&"x".to_string()),
            in_function
        );
    }
}
