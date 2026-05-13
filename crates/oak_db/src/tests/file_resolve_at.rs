use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::File;
use crate::Package;
use crate::PackageOrigin;
use crate::Script;
use crate::SourceNode;

fn make_file(db: &mut TestDb, path: &str, contents: &str) -> File {
    intern_file(db, file_url(path), contents.to_string(), None)
}

fn make_script(db: &mut TestDb, path: &str, contents: &str) -> Script {
    let file = make_file(db, path, contents);
    let script = Script::new(db, file);
    file.set_parent(db).to(Some(SourceNode::Script(script)));
    script
}

fn make_package_file(db: &mut TestDb, path: &str, contents: &str, package: Package) -> File {
    intern_file(
        db,
        file_url(path),
        contents.to_string(),
        Some(SourceNode::Package(package)),
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

#[test]
fn resolves_function_parameter_at_use_site() {
    let mut db = TestDb::new();
    let source = "f <- function(x) x\n";
    let file = make_file(&mut db, "/a.R", source);

    // Cursor on the second `x` (the use inside the function body).
    let offset = TextSize::from(source.rfind('x').unwrap() as u32);
    let resolution = file.resolve_at(&db, offset).expect("should resolve");

    assert_eq!(resolution.file, file);
    assert_eq!(resolution.name, "x");
    // Range points at the parameter declaration.
    let expected = source.find("(x)").unwrap() + 1;
    assert_eq!(usize::from(resolution.range.start()), expected);
}

#[test]
fn resolves_local_let_inside_function() {
    let mut db = TestDb::new();
    let source = "f <- function() {\n  y <- 1\n  y\n}\n";
    let file = make_file(&mut db, "/a.R", source);

    // Cursor on the second `y` (the use after the local binding).
    let offset = TextSize::from(source.rfind('y').unwrap() as u32);
    let resolution = file.resolve_at(&db, offset).expect("should resolve");

    assert_eq!(resolution.file, file);
    assert_eq!(resolution.name, "y");
    // Range points at the `y <- 1` binding (first `y` after `function() {`).
    let expected = source.find("y <- 1").unwrap();
    assert_eq!(usize::from(resolution.range.start()), expected);
}

#[test]
fn resolves_file_top_level_binding() {
    let mut db = TestDb::new();
    let source = "x <- 1\nx\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.rfind('x').unwrap() as u32);
    let resolution = file.resolve_at(&db, offset).expect("should resolve");

    assert_eq!(resolution.file, file);
    assert_eq!(resolution.name, "x");
    assert_eq!(usize::from(resolution.range.start()), 0);
}

#[test]
fn function_body_falls_through_to_file_top_level() {
    // The use is inside a function body, but the binding is at file
    // top-level. Lexical walk should reach file scope and step 2 takes
    // over.
    let mut db = TestDb::new();
    let source = "x <- 1\nf <- function() x\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.rfind('x').unwrap() as u32);
    let resolution = file.resolve_at(&db, offset).expect("should resolve");

    assert_eq!(resolution.file, file);
    assert_eq!(resolution.name, "x");
    assert_eq!(usize::from(resolution.range.start()), 0);
}

#[test]
fn resolves_source_forwarded_name_to_origin_file() {
    let mut db = TestDb::new();
    let helpers = make_script(&mut db, "/w/helpers.R", "helper <- function() 1\n");
    let analysis_source = "source(\"helpers.R\")\nhelper()\n";
    let analysis = make_file(&mut db, "/w/analysis.R", analysis_source);

    let offset = TextSize::from(analysis_source.find("helper()").unwrap() as u32);
    let resolution = analysis
        .resolve_at(&db, offset)
        .expect("should resolve via source() forwarding");

    assert_eq!(resolution.file, helpers.file(&db));
    assert_eq!(resolution.name, "helper");
}

#[test]
fn resolves_package_collation_predecessor() {
    let mut db = TestDb::new();
    let pkg = workspace_package(&db, "pkg", Some(vec!["a.R".to_string(), "b.R".to_string()]));
    db.source_graph()
        .set_workspace_packages(&mut db)
        .to(vec![pkg]);

    let a = make_package_file(&mut db, "/workspace/pkg/R/a.R", "shared <- 1\n", pkg);
    let b_source = "use_shared <- function() shared\n";
    let b = make_package_file(&mut db, "/workspace/pkg/R/b.R", b_source, pkg);

    // Cursor on `shared` inside `b`'s function body. Lexical walk finds no
    // binding in `b`, falls through to `b.resolve` which finds nothing in
    // `b`'s own exports, then walks visible imports and reaches `a` (a
    // predecessor collation file).
    let offset = TextSize::from(b_source.rfind("shared").unwrap() as u32);
    let resolution = b.resolve_at(&db, offset).expect("should resolve");

    assert_eq!(resolution.file, a);
    assert_eq!(resolution.name, "shared");
}

#[test]
fn local_after_source_shadows_forwarded_entry() {
    // R's runtime. `source()` runs first, then the local `<-` overwrites
    // the binding. `resolve_at` should land on the local.
    let mut db = TestDb::new();
    let _helpers = make_script(&mut db, "/w/helpers.R", "foo <- \"from helpers\"\n");
    let analysis_source = "source(\"helpers.R\")\nfoo <- \"local\"\nfoo\n";
    let analysis = make_file(&mut db, "/w/analysis.R", analysis_source);

    let offset = TextSize::from(analysis_source.rfind("foo").unwrap() as u32);
    let resolution = analysis.resolve_at(&db, offset).expect("should resolve");

    assert_eq!(resolution.file, analysis);
    assert_eq!(resolution.name, "foo");
    let expected = analysis_source.find("foo <- \"local\"").unwrap();
    assert_eq!(usize::from(resolution.range.start()), expected);
}

#[test]
fn source_after_local_overrides_local() {
    // R's runtime. The local `<-` binds first, then `source()`
    // overwrites by re-binding `foo`. `resolve_at` should chase to the
    // sourced file.
    let mut db = TestDb::new();
    let helpers = make_script(&mut db, "/w/helpers.R", "foo <- \"from helpers\"\n");
    let analysis_source = "foo <- \"local\"\nsource(\"helpers.R\")\nfoo\n";
    let analysis = make_file(&mut db, "/w/analysis.R", analysis_source);

    let offset = TextSize::from(analysis_source.rfind("foo").unwrap() as u32);
    let resolution = analysis.resolve_at(&db, offset).expect("should resolve");

    assert_eq!(resolution.file, helpers.file(&db));
    assert_eq!(resolution.name, "foo");
}

#[test]
fn unbound_name_returns_none() {
    let mut db = TestDb::new();
    let source = "nope\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(0);
    assert!(file.resolve_at(&db, offset).is_none());
}

#[test]
fn offset_not_on_any_use_returns_none() {
    let mut db = TestDb::new();
    let source = "x <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    // Cursor on the `<-` operator, not on any identifier use.
    let offset = TextSize::from(source.find("<-").unwrap() as u32);
    assert!(file.resolve_at(&db, offset).is_none());
}
