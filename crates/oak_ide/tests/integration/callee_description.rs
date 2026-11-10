use aether_path::FilePath;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_db::File;
use oak_db::Formal;
use oak_db::OakDatabase;
use oak_db::SourceDb;
use oak_ide::callee_description;
use oak_ide::CalleeLookup;
use oak_package_metadata::namespace::Namespace;
use oak_scan::DbScan;
use salsa::Setter;

use crate::support::install_library_package;
use crate::support::install_library_package_files;
use crate::support::install_workspace_package;
use crate::support::install_workspace_package_files;
use crate::support::place_in_workspace_scripts;
use crate::support::upsert;
use crate::support::workspace_url;

// Use the last textual occurrence so earlier definitions and calls do not
// determine the test's lookup position.
fn lookup<'db>(db: &'db OakDatabase, file: File, callee: &str) -> CalleeLookup<'db> {
    let needle = format!("{callee}(");
    let start = file.source_text(db).rfind(&needle).unwrap() as u32;
    let range = TextRange::at(TextSize::from(start), TextSize::from(callee.len() as u32));
    callee_description(db, file, range)
}

fn describe(db: &OakDatabase, file: File, callee: &str) -> Option<Vec<Formal>> {
    lookup(db, file, callee)
        .description
        .map(|description| description.formals)
}

fn set_package_namespace(db: &mut OakDatabase, package: &str, text: &str) {
    let package = db.package_by_name(package).unwrap();
    let namespace = Namespace::parse(text).unwrap();
    package.set_namespace_override(db).to(Some(namespace));
}

fn formal(name: &str, default: Option<&str>) -> Formal {
    Formal {
        name: name.to_string(),
        default: default.map(str::to_string),
    }
}

#[test]
fn test_function_in_same_file() {
    let mut db = OakDatabase::new();
    let file = upsert(
        &mut db,
        "test.R",
        "f <- function(x, ..., `my arg` = c(\"a\", 'b'), n = 1e3) NULL\nf()\n",
    );

    assert_eq!(
        describe(&db, file, "f"),
        Some(vec![
            formal("x", None),
            formal("...", None),
            formal("my arg", Some("c(\"a\", 'b')")),
            formal("n", Some("1e3")),
        ])
    );
}

#[test]
fn test_lambda() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "f <- \\(x, y = 2) x\nf()\n");

    assert_eq!(
        describe(&db, file, "f"),
        Some(vec![formal("x", None), formal("y", Some("2"))])
    );
}

#[test]
fn test_multiline_default_keeps_source_text() {
    let mut db = OakDatabase::new();
    let file = upsert(
        &mut db,
        "test.R",
        "f <- function(x = c(1,\n  2)) NULL\nf()\n",
    );

    assert_eq!(
        describe(&db, file, "f"),
        Some(vec![formal("x", Some("c(1,\n  2)"))])
    );
}

#[test]
fn test_function_in_workspace_package_sibling() {
    let mut db = OakDatabase::new();
    let files = install_workspace_package_files(&mut db, "pkg", &[], &[
        ("a.R", "f <- function(x, y = NULL) NULL\n"),
        ("b.R", "g <- function() f()\n"),
    ]);

    assert_eq!(
        describe(&db, files[1], "f"),
        Some(vec![formal("x", None), formal("y", Some("NULL"))])
    );
}

#[test]
fn test_namespace_access_from_fetched_sources() {
    let mut db = OakDatabase::new();
    install_library_package(
        &mut db,
        "pkg",
        &["exported"],
        "a.R",
        "exported <- function(data, n = 10L) NULL\ninternal <- function(z) NULL\n",
    );
    let file = upsert(
        &mut db,
        "test.R",
        "pkg::exported()\npkg::internal()\npkg:::internal()\n",
    );

    assert_eq!(
        describe(&db, file, "pkg::exported"),
        Some(vec![formal("data", None), formal("n", Some("10L"))])
    );
    assert_eq!(describe(&db, file, "pkg::internal"), None);
    assert_eq!(
        describe(&db, file, "pkg:::internal"),
        Some(vec![formal("z", None)])
    );
}

#[test]
fn test_namespace_access_before_sources_are_fetched() {
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "pkg", &["exported"], &[]);
    let file = upsert(&mut db, "test.R", "pkg::exported()\n");

    assert_eq!(describe(&db, file, "pkg::exported"), None);
}

#[test]
fn test_primitive_stubs() {
    // `base` has no `NAMESPACE`, and its primitives come from stub files.
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "base", &[], &[(
        "primitives.R",
        "sum <- function(..., na.rm = FALSE) NULL\n`[` <- .Primitive(\"[\")\n",
    )]);
    let file = upsert(&mut db, "test.R", "sum()\nbase::sum()\n`[`()\n");

    let expected = Some(vec![formal("...", None), formal("na.rm", Some("FALSE"))]);
    assert_eq!(describe(&db, file, "sum"), expected);
    assert_eq!(describe(&db, file, "base::sum"), expected);

    // A `.Primitive()` stub has no function definition to read formals from.
    assert_eq!(describe(&db, file, "`[`"), None);
}

#[test]
fn test_unresolvable_callees() {
    let mut db = OakDatabase::new();
    let file = upsert(
        &mut db,
        "test.R",
        "\
g <- function(x) NULL
h <- g
v <- 1
unknown()
h()
v()
obj$g()
g()()
nopkg::g()
",
    );

    assert_eq!(describe(&db, file, "unknown"), None);
    assert_eq!(describe(&db, file, "h"), None);
    assert_eq!(describe(&db, file, "v"), None);
    assert_eq!(describe(&db, file, "obj$g"), None);
    assert_eq!(describe(&db, file, "g()"), None);
    assert_eq!(describe(&db, file, "nopkg::g"), None);
}

#[test]
fn test_parameter_callee() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "function(f) f()\n");

    assert_eq!(describe(&db, file, "f"), None);
}

#[test]
fn test_several_definitions_must_agree() {
    let mut db = OakDatabase::new();
    let file = upsert(
        &mut db,
        "test.R",
        "\
if (cond) f <- function(a) 1 else f <- function(a) 2
f()
if (cond) g <- function(a) 1 else g <- function(b) 2
g()
if (cond) h <- function(a) 1 else h <- 1
h()
",
    );

    assert_eq!(describe(&db, file, "f"), Some(vec![formal("a", None)]));
    assert_eq!(describe(&db, file, "g"), None);
    assert_eq!(describe(&db, file, "h"), None);
}

#[test]
fn test_local_definition_shadows_global() {
    let mut db = OakDatabase::new();
    let file = upsert(
        &mut db,
        "test.R",
        "f <- function(a) NULL\nouter <- function() {\n  f <- function(b) NULL\n  f()\n}\n",
    );

    assert_eq!(describe(&db, file, "f"), Some(vec![formal("b", None)]));
}

#[test]
fn test_reexport_reports_its_pending_source() {
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "source", &["foo"], &[]);
    install_library_package_files(&mut db, "reexport", &[], &[]);
    set_package_namespace(
        &mut db,
        "reexport",
        "importFrom(source, foo)\nexport(foo)\n",
    );
    let reexport = db.package_by_name("reexport").unwrap();
    let source = db.package_by_name("source").unwrap();
    let file = upsert(
        &mut db,
        "test.R",
        "library(reexport)\nfoo()\nreexport::foo()\n",
    );

    // `source`'s pending sources can still supply the formals.
    for callee in ["foo", "reexport::foo"] {
        let lookup = lookup(&db, file, callee);
        assert_eq!(lookup.description, None);
        assert_eq!(lookup.packages, vec![reexport, source]);
    }
}

#[test]
fn test_reexport_is_documented_by_its_source() {
    let mut db = OakDatabase::new();
    install_library_package(
        &mut db,
        "source",
        &["foo"],
        "foo.R",
        "foo <- function(x) NULL\n",
    );
    install_library_package_files(&mut db, "reexport", &[], &[]);
    set_package_namespace(
        &mut db,
        "reexport",
        "importFrom(source, foo)\nexport(foo)\n",
    );
    let reexport = db.package_by_name("reexport").unwrap();
    let source = db.package_by_name("source").unwrap();
    let file = upsert(
        &mut db,
        "test.R",
        "library(reexport)\nfoo()\nreexport::foo()\n",
    );

    for callee in ["foo", "reexport::foo"] {
        let lookup = lookup(&db, file, callee);
        let description = lookup.description.unwrap();
        assert_eq!(description.formals, vec![formal("x", None)]);
        assert_eq!(description.package, Some(source));
        assert_eq!(lookup.packages, vec![reexport, source]);
    }
}

#[test]
fn test_script_function_shadows_sourced_package_function() {
    let mut db = OakDatabase::new();
    let pkg_file = install_workspace_package(
        &mut db,
        "pkg",
        &["foo"],
        "foo.R",
        "foo <- function(pkg) pkg\n",
    );
    let pkg = db.package_by_name("pkg").unwrap();
    let script = db.upsert_editor(
        FilePath::from_url(&workspace_url("pkg/script.R")),
        "source(\"R/foo.R\")\nfoo()\nfoo <- function(local) local\nfoo()\n".to_string(),
    );

    let lookup = lookup(&db, script, "foo");
    let description = lookup.description.unwrap();
    assert_eq!(description.formals, vec![formal("local", None)]);
    assert_eq!(description.definitions[0].file(&db), script);
    assert_eq!(description.package, None);
    assert_eq!(lookup.packages, vec![]);

    // Before the script's own definition, the sourced one applies.
    let start = TextSize::from(script.source_text(&db).find("foo(").unwrap() as u32);
    let range = TextRange::at(start, TextSize::from(3));
    let description = callee_description(&db, script, range).description.unwrap();
    assert_eq!(description.formals, vec![formal("pkg", None)]);
    assert_eq!(description.definitions[0].file(&db), pkg_file);
    assert_eq!(description.package, Some(pkg));
}

#[test]
fn test_function_body_call_before_local_definition_resolves_to_package() {
    let mut db = OakDatabase::new();
    install_library_package(
        &mut db,
        "pkg",
        &["f"],
        "f.R",
        "f <- function(pkg_arg) NULL\n",
    );
    let pkg = db.package_by_name("pkg").unwrap();
    let file = upsert(
        &mut db,
        "test.R",
        "library(pkg)\ng <- function() {\n  f()\n  f <- function(x) 1\n}\n",
    );

    let lookup = lookup(&db, file, "f");
    let description = lookup.description.unwrap();
    assert_eq!(description.formals, vec![formal("pkg_arg", None)]);
    assert_eq!(description.package, Some(pkg));
    assert_eq!(lookup.packages, vec![pkg]);
}

#[test]
fn test_export_pattern_package_without_sources_is_reported() {
    // `foo` might match `pkg`'s export pattern once its sources arrive.
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "pkg", &[], &[]);
    set_package_namespace(&mut db, "pkg", "exportPattern(\"^[^.]\")\n");
    let pkg = db.package_by_name("pkg").unwrap();
    let file = upsert(&mut db, "test.R", "library(pkg)\nfoo()\n");

    let lookup = lookup(&db, file, "foo");
    assert_eq!(lookup.description, None);
    assert_eq!(lookup.packages, vec![pkg]);
}

#[test]
fn test_packages_stop_at_the_binding_layer() {
    // `b` is attached last, so lookup finds `f` there and never reads `a`.
    let mut db = OakDatabase::new();
    install_library_package(&mut db, "a", &["f"], "f.R", "f <- function(a) NULL\n");
    install_library_package(&mut db, "b", &["f"], "f.R", "f <- function(b) NULL\n");
    install_library_package_files(&mut db, "pending", &[], &[]);
    let b = db.package_by_name("b").unwrap();
    let pending = db.package_by_name("pending").unwrap();
    let file = upsert(
        &mut db,
        "test.R",
        "library(a)\nlibrary(b)\nlibrary(pending)\nf()\n",
    );

    let lookup = lookup(&db, file, "f");
    assert_eq!(lookup.description.unwrap().formals, vec![formal("b", None)]);
    assert_eq!(lookup.packages, vec![pending, b]);
}

#[test]
fn test_namespace_import_layers_report_their_package() {
    for namespace in ["importFrom(lib, foo)\n", "import(lib)\n"] {
        let mut db = OakDatabase::new();
        install_library_package(
            &mut db,
            "lib",
            &["foo"],
            "foo.R",
            "foo <- function(x) NULL\n",
        );
        let lib = db.package_by_name("lib").unwrap();
        let file = install_workspace_package(&mut db, "pkg", &[], "a.R", "g <- function() foo()\n");
        set_package_namespace(&mut db, "pkg", namespace);

        let lookup = lookup(&db, file, "foo");
        let description = lookup.description.unwrap();
        assert_eq!(description.formals, vec![formal("x", None)]);
        assert_eq!(description.package, Some(lib));
        assert_eq!(lookup.packages, vec![lib]);
    }
}

#[test]
fn test_sourcing_file_attach_reports_its_package() {
    // `helpers.R` attaches nothing itself; `main.R` attaches `lib` before
    // sourcing it.
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "lib", &["foo"], &[]);
    let lib = db.package_by_name("lib").unwrap();
    let main = upsert(&mut db, "main.R", "library(lib)\nsource(\"helpers.R\")\n");
    let helpers = upsert(&mut db, "helpers.R", "foo()\n");
    place_in_workspace_scripts(&mut db, vec![main, helpers]);

    let lookup = lookup(&db, helpers, "foo");
    assert_eq!(lookup.description, None);
    assert_eq!(lookup.packages, vec![lib]);
}

#[test]
fn test_workspace_package_is_not_reported() {
    // Its files are scanned, so nothing about it is pending.
    let mut db = OakDatabase::new();
    install_workspace_package(
        &mut db,
        "pkg",
        &["foo"],
        "foo.R",
        "foo <- function(x) NULL\n",
    );
    let file = upsert(&mut db, "test.R", "library(pkg)\nfoo()\npkg::foo()\n");

    for callee in ["foo", "pkg::foo"] {
        let lookup = lookup(&db, file, callee);
        assert_eq!(lookup.description.unwrap().formals, vec![formal("x", None)]);
        assert_eq!(lookup.packages, vec![]);
    }
}

#[test]
fn test_lexical_definition_consults_no_package() {
    let mut db = OakDatabase::new();
    install_library_package(&mut db, "pkg", &["f"], "f.R", "f <- function(a) NULL\n");
    let file = upsert(
        &mut db,
        "test.R",
        "library(pkg)\ng <- function() {\n  f <- function(b) NULL\n  f()\n}\n",
    );

    let lookup = lookup(&db, file, "f");
    assert_eq!(lookup.description.unwrap().formals, vec![formal("b", None)]);
    assert_eq!(lookup.packages, vec![]);
}
