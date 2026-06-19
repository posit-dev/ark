use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::Definition;
use crate::File;
use crate::Package;
use crate::Root;

fn make_file(db: &mut TestDb, path: &str, contents: &str) -> File {
    File::new(db, file_path(path), contents.to_string(), None)
}

/// Resolve at `offset`, asserting exactly one definition. Most cases are
/// unambiguous; the ambiguous ones (e.g. `if`/`else`) have their own test.
fn resolve_one(db: &TestDb, file: File, offset: TextSize) -> Definition<'_> {
    let defs = file.resolve_at(db, offset);
    assert_eq!(defs.len(), 1);
    defs[0]
}

fn make_package_file(db: &mut TestDb, path: &str, contents: &str, package: Package) -> File {
    File::new(db, file_path(path), contents.to_string(), Some(package))
}

/// Set up a workspace root with the given scripts (top-level files with
/// `package == None`). Returns the file handles. Registers the root on
/// `WorkspaceRoots` so `file_by_path` can resolve `source()` targets.
fn setup_workspace_scripts(
    db: &mut TestDb,
    workspace_path: &str,
    scripts: &[(&str, &str)],
) -> (Root, Vec<File>) {
    let root = workspace_root(db, workspace_path);
    let files: Vec<File> = scripts
        .iter()
        .map(|(path, contents)| make_file(db, path, contents))
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    (root, files)
}

fn install_workspace_package(db: &mut TestDb, name: &str) -> (Root, Package) {
    let root = workspace_root(db, &format!("workspace/{name}"));
    let pkg = Package::new(
        db,
        file_path(&format!("workspace/{name}/DESCRIPTION")),
        name.to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );
    root.set_packages(db).to(vec![pkg]);
    db.workspace_roots().set_roots(db).to(vec![root]);
    (root, pkg)
}

#[test]
fn test_resolves_function_parameter_at_use_site() {
    let mut db = TestDb::new();
    let source = "f <- function(x) x\n";
    let file = make_file(&mut db, "a.R", source);

    // Cursor on the second `x` (the use inside the function body).
    let offset = TextSize::from(source.rfind('x').unwrap() as u32);
    let def = resolve_one(&db, file, offset);

    assert_eq!(def.file(&db), file);
    assert_eq!(def.name(&db).text(&db).as_str(), "x");
    // Range points at the parameter declaration.
    let expected = source.find("(x)").unwrap() + 1;
    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), expected);
}

#[test]
fn test_resolves_local_let_inside_function() {
    let mut db = TestDb::new();
    let source = "f <- function() {\n  y <- 1\n  y\n}\n";
    let file = make_file(&mut db, "a.R", source);

    // Cursor on the second `y` (the use after the local binding).
    let offset = TextSize::from(source.rfind('y').unwrap() as u32);
    let def = resolve_one(&db, file, offset);

    assert_eq!(def.file(&db), file);
    assert_eq!(def.name(&db).text(&db).as_str(), "y");
    // Range points at the `y <- 1` binding.
    let expected = source.find("y <- 1").unwrap();
    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), expected);
}

#[test]
fn test_resolves_file_top_level_binding() {
    let mut db = TestDb::new();
    let source = "x <- 1\nx\n";
    let file = make_file(&mut db, "a.R", source);

    let offset = TextSize::from(source.rfind('x').unwrap() as u32);
    let def = resolve_one(&db, file, offset);

    assert_eq!(def.file(&db), file);
    assert_eq!(def.name(&db).text(&db).as_str(), "x");
    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), 0);
}

#[test]
fn test_function_body_falls_through_to_file_top_level() {
    // The use is inside a function body, but the binding is at file
    // top-level. Lexical walk should reach file scope and step 2 takes
    // over.
    let mut db = TestDb::new();
    let source = "x <- 1\nf <- function() x\n";
    let file = make_file(&mut db, "a.R", source);

    let offset = TextSize::from(source.rfind('x').unwrap() as u32);
    let def = resolve_one(&db, file, offset);

    assert_eq!(def.file(&db), file);
    assert_eq!(def.name(&db).text(&db).as_str(), "x");
    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), 0);
}

#[test]
fn test_resolves_source_forwarded_name_to_origin_file() {
    let mut db = TestDb::new();
    let (_root, files) = setup_workspace_scripts(&mut db, "w", &[
        ("w/helpers.R", "helper <- function() 1\n"),
        ("w/analysis.R", "source(\"helpers.R\")\nhelper()\n"),
    ]);
    let helpers = files[0];
    let analysis = files[1];

    let analysis_source = analysis.contents(&db).clone();
    let offset = TextSize::from(analysis_source.find("helper()").unwrap() as u32);
    let def = resolve_one(&db, analysis, offset);

    assert_eq!(def.file(&db), helpers);
    assert_eq!(def.name(&db).text(&db).as_str(), "helper");
}

#[test]
fn test_resolves_package_sibling_predecessor() {
    let mut db = TestDb::new();
    let (_root, pkg) = install_workspace_package(&mut db, "pkg");

    let a = make_package_file(&mut db, "workspace/pkg/R/a.R", "shared <- 1\n", pkg);
    let b_source = "use_shared <- function() shared\n";
    let b = make_package_file(&mut db, "workspace/pkg/R/b.R", b_source, pkg);
    pkg.set_files(&mut db).to(vec![a, b]);

    // Cursor on `shared` inside `b`'s function body. Lexical walk finds
    // no binding in `b`, falls through to `b.resolve` which finds nothing
    // in `b`'s own exports, then walks visible imports and reaches `a`
    // (a predecessor sibling).
    let offset = TextSize::from(b_source.rfind("shared").unwrap() as u32);
    let def = resolve_one(&db, b, offset);

    assert_eq!(def.file(&db), a);
    assert_eq!(def.name(&db).text(&db).as_str(), "shared");
}

#[test]
fn test_local_after_source_shadows_forwarded_entry() {
    // R's runtime. `source()` runs first, then the local `<-` overwrites
    // the binding. `resolve_at` should land on the local.
    let mut db = TestDb::new();
    let (_root, files) = setup_workspace_scripts(&mut db, "w", &[
        ("w/helpers.R", "foo <- \"from helpers\"\n"),
        (
            "w/analysis.R",
            "source(\"helpers.R\")\nfoo <- \"local\"\nfoo\n",
        ),
    ]);
    let analysis = files[1];

    let analysis_source = analysis.contents(&db).clone();
    let offset = TextSize::from(analysis_source.rfind("foo").unwrap() as u32);
    let def = resolve_one(&db, analysis, offset);

    assert_eq!(def.file(&db), analysis);
    assert_eq!(def.name(&db).text(&db).as_str(), "foo");
    let expected = analysis_source.find("foo <- \"local\"").unwrap();
    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), expected);
}

#[test]
fn test_source_after_local_overrides_local() {
    // R's runtime. The local `<-` binds first, then `source()`
    // overwrites by re-binding `foo`. `resolve_at` should chase to the
    // sourced file.
    let mut db = TestDb::new();
    let (_root, files) = setup_workspace_scripts(&mut db, "w", &[
        ("w/helpers.R", "foo <- \"from helpers\"\n"),
        (
            "w/analysis.R",
            "foo <- \"local\"\nsource(\"helpers.R\")\nfoo\n",
        ),
    ]);
    let helpers = files[0];
    let analysis = files[1];

    let analysis_source = analysis.contents(&db).clone();
    let offset = TextSize::from(analysis_source.rfind("foo").unwrap() as u32);
    let def = resolve_one(&db, analysis, offset);

    assert_eq!(def.file(&db), helpers);
    assert_eq!(def.name(&db).text(&db).as_str(), "foo");
}

#[test]
fn test_unbound_name_returns_none() {
    let mut db = TestDb::new();
    let source = "nope\n";
    let file = make_file(&mut db, "a.R", source);

    let offset = TextSize::from(0);
    assert!(file.resolve_at(&db, offset).is_empty());
}

#[test]
fn test_offset_not_on_any_use_returns_none() {
    let mut db = TestDb::new();
    let source = "x <- 1\n";
    let file = make_file(&mut db, "a.R", source);

    // Cursor on the `<-` operator, not on any identifier use.
    let offset = TextSize::from(source.find("<-").unwrap() as u32);
    assert!(file.resolve_at(&db, offset).is_empty());
}

#[test]
fn test_top_level_use_between_defs_binds_reaching_def() {
    // A use sitting between two top-level defs of the same name binds to the
    // earlier (reaching) def, not the later one. The EOF `exports()` view
    // would wrongly pick `foo <- 2`.
    let mut db = TestDb::new();
    let source = "foo <- 1\nfoo\nfoo <- 2\n";
    let file = make_file(&mut db, "a.R", source);

    let offset = TextSize::from(source.find("\nfoo\n").unwrap() as u32 + 1);
    let def = resolve_one(&db, file, offset);

    assert_eq!(def.file(&db), file);
    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), source.find("foo <- 1").unwrap());
}

#[test]
fn test_top_level_use_after_redefinition_binds_latest_def() {
    // A use after both defs binds to the latest one, the same answer the EOF
    // view gives. Guards against over-correcting the reaching-def fix.
    let mut db = TestDb::new();
    let source = "foo <- 1\nfoo <- 2\nfoo\n";
    let file = make_file(&mut db, "a.R", source);

    let offset = TextSize::from(source.rfind("foo").unwrap() as u32);
    let def = resolve_one(&db, file, offset);

    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), source.find("foo <- 2").unwrap());
}

#[test]
fn test_cursor_on_assignment_target_resolves_to_itself() {
    // Cursor on the `foo` being bound, not a use of it: navigate to self.
    let mut db = TestDb::new();
    let source = "foo <- 1\n";
    let file = make_file(&mut db, "a.R", source);

    let def = resolve_one(&db, file, TextSize::from(0));

    assert_eq!(def.file(&db), file);
    assert_eq!(def.name(&db).text(&db).as_str(), "foo");
    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(usize::from(range.start()), 0);
}

#[test]
fn test_cursor_on_parameter_declaration_resolves_to_itself() {
    let mut db = TestDb::new();
    let source = "f <- function(x) 1\n";
    let file = make_file(&mut db, "a.R", source);

    // Cursor on the `x` parameter declaration.
    let offset = TextSize::from(source.find("(x)").unwrap() as u32 + 1);
    let def = resolve_one(&db, file, offset);

    let range = def.name_range(&db).expect("local has a name range");
    assert_eq!(range.start(), offset);
}

#[test]
fn test_top_level_conditional_reports_both_arm_defs() {
    // A name defined on both arms of an `if`/`else` is ambiguous at the use:
    // either arm could have run, so both defs are reported.
    let mut db = TestDb::new();
    let source = "if (cond) foo <- 1 else foo <- 2\nfoo\n";
    let file = make_file(&mut db, "a.R", source);

    let offset = TextSize::from(source.rfind("foo").unwrap() as u32);
    let defs = file.resolve_at(&db, offset);

    let starts: Vec<usize> = defs
        .iter()
        .map(|d| usize::from(d.name_range(&db).expect("local has a name range").start()))
        .collect();
    assert_eq!(defs.len(), 2);
    assert!(starts.contains(&source.find("foo <- 1").unwrap()));
    assert!(starts.contains(&source.find("foo <- 2").unwrap()));
}

// Package-layer resolution, pending. `resolve` / `resolve_at` walk only
// `ImportLayer::File`; the `From` / `Package` layers (`library()`, NAMESPACE
// `importFrom`, the base search path) are deferred, see the TODOs in
// `file_resolve.rs`. These scenarios came from the old goto-def `LegacyDb`
// suite, which only asserted them as unsupported (resolved to `None`). When
// package layers land, add them here on `resolve_at`:
//
// - `library(pkg)` in a script makes a `pkg` export resolve at a later use.
// - `importFrom(dplyr, mutate)` in a package's NAMESPACE makes `mutate` resolve.
// - a package file resolves base symbols (e.g. `cat`).
// - a standalone script resolves base / default-attached symbols.
// - a script's search path is identical at top level and in a function body.
// - `library()` attached inside a sourced file is visible to a function body
//   that runs after the `source()`.
//
// Resolving package symbols also needs a navigable location for installed
// package files, which aren't `oak_db::File` entities yet.
