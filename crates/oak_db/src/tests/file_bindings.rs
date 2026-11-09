use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use stdext::SortedVec;

use crate::test_path::file_path;
use crate::tests::test_db::make_package;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::BindingKind;
use crate::Bindings;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;

fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "w");
    let files: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| {
            File::new(
                db,
                file_path(name),
                FileRevision::zero(),
                Some(contents.to_string()),
                None,
            )
        })
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    files
}

fn kinds(bindings: &Bindings) -> Vec<(&str, BindingKind)> {
    bindings.iter().collect()
}

#[test]
fn test_file_bindings_classify_function_values() {
    let mut db = TestDb::new();
    let source = "\
f <- function(x) x
lambda <- \\(x) x
`[` <- .Primitive(\"[\")
.Primitive(\"c\") -> right
assign(\"assigned\", function() 1)
";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);

    assert_eq!(kinds(files[0].bindings(&db)), vec![
        ("[", BindingKind::Function),
        ("assigned", BindingKind::Function),
        ("f", BindingKind::Function),
        ("lambda", BindingKind::Function),
        ("right", BindingKind::Function),
    ]);
}

#[test]
fn test_file_bindings_other_values_are_unknown() {
    let mut db = TestDb::new();
    let source = "\
x <- 1
alias <- f
wrapped <- memoise(function() 1)
assign(\"assigned\", 1)
";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);

    assert_eq!(kinds(files[0].bindings(&db)), vec![
        ("alias", BindingKind::Unknown),
        ("assigned", BindingKind::Unknown),
        ("wrapped", BindingKind::Unknown),
        ("x", BindingKind::Unknown),
    ]);
}

#[test]
fn test_file_bindings_are_function_only_if_every_definition_is() {
    let mut db = TestDb::new();
    let source = "\
if (cond) both <- function() 1 else both <- function() 2
if (cond) mixed <- function() 1 else mixed <- 2
";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);

    assert_eq!(kinds(files[0].bindings(&db)), vec![
        ("both", BindingKind::Function),
        ("mixed", BindingKind::Unknown),
    ]);
}

#[test]
fn test_file_bindings_match_exports_names() {
    // A name forwarded by `source()` has unknown kind. A later local
    // definition shadows the forwarded binding and supplies its kind.
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/helpers.R", "helper <- function() 1\nshared <- 1\n"),
        (
            "w/analysis.R",
            "source(\"helpers.R\")\nshared <- function() 2\n",
        ),
    ]);
    let analysis = files[1];

    let mut export_names: Vec<&str> = analysis.exports(&db).iter().map(|(name, _)| name).collect();
    export_names.sort();
    let binding_names: Vec<&str> = analysis
        .bindings(&db)
        .iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(binding_names, export_names);

    assert_eq!(kinds(analysis.bindings(&db)), vec![
        ("helper", BindingKind::Unknown),
        ("shared", BindingKind::Function),
    ]);
}

#[test]
fn test_package_bindings_merge_files() {
    let mut db = TestDb::new();
    let (pkg, _files) = make_package(&mut db, "pkg", Namespace::default(), &[
        (
            "ws/pkg/R/a.R",
            "f <- function() 1\ng <- function() 1\nx <- 1\n",
        ),
        ("ws/pkg/R/b.R", "f <- function() 2\ng <- 2\n"),
    ]);

    assert_eq!(kinds(pkg.bindings(&db)), vec![
        ("f", BindingKind::Function),
        ("g", BindingKind::Unknown),
        ("x", BindingKind::Unknown),
    ]);
}

#[test]
fn test_package_bindings_exclude_reexports() {
    // `tibble` is re-exported through `importFrom()` with no local binding.
    let mut db = TestDb::new();
    let namespace = Namespace {
        exports: SortedVec::from_vec(vec!["own".to_string(), "tibble".to_string()]),
        imports: vec![Import {
            name: "tibble".to_string(),
            package: "tibble".to_string(),
        }],
        ..Default::default()
    };
    let (pkg, _files) = make_package(&mut db, "pkg", namespace, &[(
        "ws/pkg/R/a.R",
        "own <- function() 1\n",
    )]);

    assert_eq!(kinds(pkg.bindings(&db)), vec![(
        "own",
        BindingKind::Function
    )]);
}
