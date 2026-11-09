use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_db::File;
use oak_db::Formal;
use oak_db::OakDatabase;
use oak_ide::callee_description;

use crate::support::install_library_package;
use crate::support::install_library_package_files;
use crate::support::install_workspace_package_files;
use crate::support::upsert;

// Use the last textual occurrence so earlier definitions and calls do not
// determine the test's lookup position.
fn describe(db: &OakDatabase, file: File, callee: &str) -> Option<Vec<Formal>> {
    let needle = format!("{callee}(");
    let start = file.source_text(db).rfind(&needle).unwrap() as u32;
    let range = TextRange::at(TextSize::from(start), TextSize::from(callee.len() as u32));
    callee_description(db, file, range).map(|description| description.formals)
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
