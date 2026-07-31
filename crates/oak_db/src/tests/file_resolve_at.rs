use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use stdext::SortedVec;

use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::Definition;
use crate::File;
use crate::FileRevision;
use crate::Package;
use crate::Root;
use crate::RootKind;

fn make_file(db: &mut TestDb, path: &str, contents: &str) -> File {
    File::new(
        db,
        file_path(path),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

/// Resolve at `offset`, asserting exactly one definition. Most cases are
/// unambiguous; the ambiguous ones (e.g. `if`/`else`) have their own test.
fn resolve_one(db: &TestDb, file: File, offset: TextSize) -> Definition<'_> {
    let defs = file.resolve_at(db, offset);
    assert_eq!(defs.len(), 1);
    defs[0]
}

fn make_package_file(db: &mut TestDb, path: &str, contents: &str, package: Package) -> File {
    File::new(
        db,
        file_path(path),
        FileRevision::zero(),
        Some(contents.to_string()),
        Some(package),
    )
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
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
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

    let analysis_source = analysis.source_text(&db).clone();
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

    let analysis_source = analysis.source_text(&db).clone();
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

    let analysis_source = analysis.source_text(&db).clone();
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

// Package-layer resolution, remaining items. These need either installed-package
// files as `oak_db::File` entities (for navigable locations) or a broader test
// infrastructure:
//
// - `importFrom(dplyr, mutate)` in a package's NAMESPACE makes `mutate` resolve.
// - a package file resolves base symbols (e.g. `cat`).
// - a standalone script resolves base / default-attached symbols.
// - a script's search path is identical at top level and in a function body.
// - `library()` attached inside a sourced file is visible to a function body
//   that runs after the `source()`.

fn install_library_package(
    db: &mut TestDb,
    name: &str,
    exports: &[&str],
    files: &[(&str, &str)],
) -> (Root, Package) {
    install_package(db, RootKind::Library, name, exports, files)
}

fn install_package(
    db: &mut TestDb,
    kind: RootKind,
    name: &str,
    exports: &[&str],
    files: &[(&str, &str)],
) -> (Root, Package) {
    let prefix = match kind {
        RootKind::Library => "library",
        RootKind::Workspace => "workspace",
    };
    let root = Root::new(
        db,
        file_path(&format!("{prefix}/{name}")),
        kind,
        vec![],
        vec![],
    );
    let namespace = Namespace {
        exports: SortedVec::from_vec(exports.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    let pkg = Package::new(
        db,
        file_path(&format!("{prefix}/{name}/DESCRIPTION")),
        name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let pkg_files: Vec<File> = files
        .iter()
        .map(|(path, contents)| {
            File::new(
                db,
                file_path(path),
                FileRevision::zero(),
                Some(contents.to_string()),
                Some(pkg),
            )
        })
        .collect();
    pkg.set_files(db).to(pkg_files);
    root.set_packages(db).to(vec![pkg]);
    match kind {
        RootKind::Library => db.library_roots().set_roots(db).to(vec![root]),
        RootKind::Workspace => db.workspace_roots().set_roots(db).to(vec![root]),
    };
    (root, pkg)
}

#[test]
fn test_library_call_makes_pkg_export_resolve() {
    // `library(mypkg)` in a script attaches `mypkg` as a `Package` layer.
    // A later bare use of an exported name should resolve to the binding
    // in `mypkg`'s file.
    let mut db = TestDb::new();
    let (_root, pkg) = install_library_package(&mut db, "mypkg", &["foo"], &[(
        "library/mypkg/R/a.R",
        "foo <- function() 42\n",
    )]);
    let pkg_file = pkg.files(&db)[0];

    let (_ws_root, files) =
        setup_workspace_scripts(&mut db, "ws", &[("ws/script.R", "library(mypkg)\nfoo\n")]);
    let script = files[0];
    let source = script.source_text(&db).clone();

    // Cursor on `foo` (the use after `library(mypkg)`).
    let offset = TextSize::from(source.rfind("foo").unwrap() as u32);
    let def = resolve_one(&db, script, offset);

    assert_eq!(def.file(&db), pkg_file);
    assert_eq!(def.name(&db).text(&db).as_str(), "foo");
}

#[test]
fn test_library_call_makes_workspace_pkg_export_resolve() {
    // Same as `test_library_call_makes_pkg_export_resolve` but `mypkg` lives
    // in the workspace rather than an installed library. `package_by_name`
    // walks both workspace and library roots, so the resolution path is the
    // same for both.
    let mut db = TestDb::new();
    let (_root, pkg) = install_package(&mut db, RootKind::Workspace, "mypkg", &["foo"], &[(
        "workspace/mypkg/R/a.R",
        "foo <- function() 42\n",
    )]);
    let pkg_file = pkg.files(&db)[0];

    // Script is a floating file (no root registration needed for `imports()`
    // to find workspace packages via `package_by_name`).
    let source = "library(mypkg)\nfoo\n";
    let script = make_file(&mut db, "ws/script.R", source);

    let offset = TextSize::from(source.rfind("foo").unwrap() as u32);
    let def = resolve_one(&db, script, offset);

    assert_eq!(def.file(&db), pkg_file);
    assert_eq!(def.name(&db).text(&db).as_str(), "foo");
}

#[test]
fn test_namespace_import_pkg_makes_export_resolve_in_package_file() {
    // A package file whose NAMESPACE has `import(extpkg)` can resolve
    // a bare use of `bar` to `extpkg`'s file via the `Package` layer that
    // `package_imports` injects.
    let mut db = TestDb::new();
    let (_lib_root, ext_pkg) = install_library_package(&mut db, "extpkg", &["bar"], &[(
        "library/extpkg/R/b.R",
        "bar <- function() 99\n",
    )]);
    let ext_file = ext_pkg.files(&db)[0];

    // Workspace package that `import(extpkg)` in its NAMESPACE.
    let ws_root = workspace_root(&db, "workspace/mypkg");
    let ns = {
        use oak_package_metadata::namespace::Namespace;
        Namespace {
            package_imports: vec!["extpkg".to_string()],
            ..Default::default()
        }
    };
    let ws_pkg = Package::new(
        &db,
        file_path("workspace/mypkg/DESCRIPTION"),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(ns),
        Vec::new(),
        Vec::new(),
    );
    let source = "bar\n";
    let ws_file = File::new(
        &db,
        file_path("workspace/mypkg/R/a.R"),
        FileRevision::zero(),
        Some(source.to_string()),
        Some(ws_pkg),
    );
    ws_pkg.set_files(&mut db).to(vec![ws_file]);
    ws_root.set_packages(&mut db).to(vec![ws_pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![ws_root]);

    let offset = TextSize::from(0);
    let def = resolve_one(&db, ws_file, offset);

    assert_eq!(def.file(&db), ext_file);
    assert_eq!(def.name(&db).text(&db).as_str(), "bar");
}

#[test]
fn test_namespace_importfrom_makes_export_resolve_in_package_file() {
    // A package file whose NAMESPACE has `importFrom(extpkg, baz)` resolves a
    // bare use of `baz` to `extpkg`'s file via the `From` layer that
    // `package_imports` injects (symbol -> source-package map).
    use oak_package_metadata::namespace::Import;
    use oak_package_metadata::namespace::Namespace;

    let mut db = TestDb::new();
    let (_lib_root, ext_pkg) = install_library_package(&mut db, "extpkg", &["baz"], &[(
        "library/extpkg/R/b.R",
        "baz <- function() 99\n",
    )]);
    let ext_file = ext_pkg.files(&db)[0];

    let ws_root = workspace_root(&db, "workspace/mypkg");
    let ns = Namespace {
        imports: vec![Import {
            name: "baz".to_string(),
            package: "extpkg".to_string(),
        }],
        ..Default::default()
    };
    let ws_pkg = Package::new(
        &db,
        file_path("workspace/mypkg/DESCRIPTION"),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(ns),
        Vec::new(),
        Vec::new(),
    );
    let ws_file = File::new(
        &db,
        file_path("workspace/mypkg/R/a.R"),
        FileRevision::zero(),
        Some("baz\n".to_string()),
        Some(ws_pkg),
    );
    ws_pkg.set_files(&mut db).to(vec![ws_file]);
    ws_root.set_packages(&mut db).to(vec![ws_pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![ws_root]);

    let def = resolve_one(&db, ws_file, TextSize::from(0));

    assert_eq!(def.file(&db), ext_file);
    assert_eq!(def.name(&db).text(&db).as_str(), "baz");
}

#[test]
fn test_conditional_library_resolves_only_inside_its_branch() {
    // The reported symptom of the conditional-attach bug was goto-definition,
    // which reaches the attach layers through `resolve_at` rather than
    // `imports_at`. Nothing says the branch ran, so the use after it resolves
    // to nothing while the one inside the arm still lands in the package.
    let mut db = TestDb::new();
    let (_root, pkg) = install_library_package(&mut db, "mypkg", &["foo"], &[(
        "library/mypkg/R/a.R",
        "foo <- function() 42\n",
    )]);
    let pkg_file = pkg.files(&db)[0];

    let (_ws_root, files) = setup_workspace_scripts(&mut db, "ws", &[(
        "ws/script.R",
        "if (cond) {\n  library(mypkg)\n  foo\n}\nfoo\n",
    )]);
    let script = files[0];
    let source = script.source_text(&db).clone();

    let inside = TextSize::from(source.find("  foo").unwrap() as u32 + 2);
    assert_eq!(resolve_one(&db, script, inside).file(&db), pkg_file);

    let after = TextSize::from(source.rfind("foo").unwrap() as u32);
    assert!(script.resolve_at(&db, after).is_empty());
}

/// A workspace holding `main.R`, which sources `helpers.R`, plus whatever else
/// the caller lists. Returns the files in the given order.
fn setup_sourced(db: &mut TestDb, files: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "w");
    let entities: Vec<File> = files
        .iter()
        .map(|(path, contents)| make_file(db, path, contents))
        .collect();
    root.set_scripts(db).to(entities.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    entities
}

#[test]
fn test_sourced_file_sees_definitions_before_the_source_call() {
    let mut db = TestDb::new();
    let helpers_source = "before\n";
    let files = setup_sourced(&mut db, &[
        ("w/main.R", "before <- 1\nsource(\"helpers.R\")\n"),
        ("w/helpers.R", helpers_source),
    ]);
    let (main, helpers) = (files[0], files[1]);

    let def = resolve_one(&db, helpers, TextSize::from(0));
    assert_eq!(def.file(&db), main);
    assert_eq!(def.name(&db).text(&db).as_str(), "before");
}

#[test]
fn test_sourced_file_top_level_does_not_see_definitions_after_the_source_call() {
    // `after` hadn't run when `source()` executed, so `helpers.R`'s top level
    // genuinely can't see it. Without narrowing this resolves, because
    // `ImportLayer::File` goes through whole-file `exports()`.
    let mut db = TestDb::new();
    let files = setup_sourced(&mut db, &[
        ("w/main.R", "source(\"helpers.R\")\nafter <- 1\n"),
        ("w/helpers.R", "after\n"),
    ]);
    let helpers = files[1];

    assert!(helpers.resolve_at(&db, TextSize::from(0)).is_empty());
}

#[test]
fn test_sourced_file_function_body_still_sees_definitions_after_the_source_call() {
    // A function defined in `helpers.R` can be called from `main.R` after
    // `main.R` finished running, so it does see `after`. Narrowing is for the
    // Eager view only.
    let mut db = TestDb::new();
    let helpers_source = "f <- function() after\n";
    let files = setup_sourced(&mut db, &[
        ("w/main.R", "source(\"helpers.R\")\nafter <- 1\n"),
        ("w/helpers.R", helpers_source),
    ]);
    let (main, helpers) = (files[0], files[1]);

    let offset = TextSize::from(helpers_source.find("after").unwrap() as u32);
    let def = resolve_one(&db, helpers, offset);
    assert_eq!(def.file(&db), main);
}

#[test]
fn test_conditional_definition_before_the_source_call_stays_visible() {
    // `maybe` is only maybe-bound at the `source()` call. We over-approximate
    // and keep it, so an unknown-symbol diagnostic won't fire on a name that
    // might well be there.
    let mut db = TestDb::new();
    let files = setup_sourced(&mut db, &[
        ("w/main.R", "if (cond) maybe <- 1\nsource(\"helpers.R\")\n"),
        ("w/helpers.R", "maybe\n"),
    ]);
    let (main, helpers) = (files[0], files[1]);

    let def = resolve_one(&db, helpers, TextSize::from(0));
    assert_eq!(def.file(&db), main);
}

#[test]
fn test_source_call_in_a_function_body_narrows_nothing() {
    // Nothing says when (or whether) `load()` runs, so there's no program point
    // to narrow to. `helpers.R` falls back to whole-file exports and sees
    // `after`.
    let mut db = TestDb::new();
    let files = setup_sourced(&mut db, &[
        (
            "w/main.R",
            "load <- function() source(\"helpers.R\")\nafter <- 1\n",
        ),
        ("w/helpers.R", "after\n"),
    ]);
    let (main, helpers) = (files[0], files[1]);

    let def = resolve_one(&db, helpers, TextSize::from(0));
    assert_eq!(def.file(&db), main);
}

#[test]
fn test_both_sourcing_files_contribute_definitions() {
    // Two files source the same target. Each is a separate possible runtime
    // (in `a.R`'s run `foo` comes from `a.R`, in `b.R`'s run from `b.R`), so
    // both bindings are visible from the sourced file, not just the
    // alphabetically-first sourcing file.
    let mut db = TestDb::new();
    let files = setup_sourced(&mut db, &[
        ("w/a.R", "foo <- 1\nsource(\"helpers.R\")\n"),
        ("w/b.R", "foo <- 2\nsource(\"helpers.R\")\n"),
        ("w/helpers.R", "foo\n"),
    ]);
    let (a, b, helpers) = (files[0], files[1], files[2]);

    let defs = helpers.resolve_at(&db, TextSize::from(0));
    let def_files: Vec<File> = defs.iter().map(|def| def.file(&db)).collect();
    assert_eq!(def_files, vec![a, b]);
}

#[test]
fn test_offset_narrowing_applies_independently_per_sourcing_site() {
    // `a.R` binds `foo` before its `source()` call, `b.R` only after. The
    // union across sourcing files doesn't defeat the per-site narrowing: only
    // `a.R`'s binding had run by the time `helpers.R`'s top level executed in
    // `b.R`'s chain, so `b.R` contributes nothing.
    let mut db = TestDb::new();
    let files = setup_sourced(&mut db, &[
        ("w/a.R", "foo <- 1\nsource(\"helpers.R\")\n"),
        ("w/b.R", "source(\"helpers.R\")\nfoo <- 2\n"),
        ("w/helpers.R", "foo\n"),
    ]);
    let a = files[0];
    let helpers = files[2];

    let def = resolve_one(&db, helpers, TextSize::from(0));
    assert_eq!(def.file(&db), a);
}

#[test]
fn test_lazy_view_sees_both_sourcing_files() {
    // A function body runs after the whole file, so unlike the offset-narrowed
    // top-level case, it sees both sourcing files' bindings regardless of
    // where each `source()` call sits in its own file. Exercises the tracked
    // `imports_by_sourcing_file` path (via `File::resolve`), not the `_at` one.
    let mut db = TestDb::new();
    let helpers_source = "f <- function() foo\n";
    let files = setup_sourced(&mut db, &[
        ("w/a.R", "foo <- 1\nsource(\"helpers.R\")\n"),
        ("w/b.R", "foo <- 2\nsource(\"helpers.R\")\n"),
        ("w/helpers.R", helpers_source),
    ]);
    let (a, b, helpers) = (files[0], files[1], files[2]);

    let offset = TextSize::from(helpers_source.find("foo").unwrap() as u32);
    let defs = helpers.resolve_at(&db, offset);
    let def_files: Vec<File> = defs.iter().map(|def| def.file(&db)).collect();
    assert_eq!(def_files, vec![a, b]);
}

#[test]
fn test_convergent_chains_dedupe_to_one_definition() {
    // `a.R` and `b.R` both source `defs.R` before `helpers.R`, so both chains
    // reach the same `foo` binding. The union must not report it twice.
    let mut db = TestDb::new();
    let files = setup_sourced(&mut db, &[
        ("w/defs.R", "foo <- 1\n"),
        ("w/a.R", "source(\"defs.R\")\nsource(\"helpers.R\")\n"),
        ("w/b.R", "source(\"defs.R\")\nsource(\"helpers.R\")\n"),
        ("w/helpers.R", "foo\n"),
    ]);
    let defs = files[0];
    let helpers = files[3];

    let def = resolve_one(&db, helpers, TextSize::from(0));
    assert_eq!(def.file(&db), defs);
}

#[test]
fn test_narrowing_applies_at_each_hop_of_a_source_chain() {
    // `setup.R` sources `helpers.R` before binding `late_setup`, and `main.R`
    // sources `setup.R` before binding `late_main`, so `helpers.R`'s top level
    // sees neither. `early_main` ran before both calls, so it does show up.
    let mut db = TestDb::new();
    let files = setup_sourced(&mut db, &[
        (
            "w/main.R",
            "early_main <- 1\nsource(\"setup.R\")\nlate_main <- 2\n",
        ),
        ("w/setup.R", "source(\"helpers.R\")\nlate_setup <- 3\n"),
        ("w/helpers.R", "early_main\n"),
    ]);
    let (main, helpers) = (files[0], files[2]);

    let def = resolve_one(&db, helpers, TextSize::from(0));
    assert_eq!(def.file(&db), main);

    for name in ["late_main", "late_setup"] {
        helpers
            .set_source_text_override(&mut db)
            .to(Some(format!("{name}\n")));
        assert!(helpers.resolve_at(&db, TextSize::from(0)).is_empty());
    }
}
