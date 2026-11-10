use oak_db::BindingKind;
use oak_db::File;
use oak_db::Name;
use oak_db::NamespaceVisibility;
use oak_db::OakDatabase;
use oak_db::SourceDb;
use oak_ide::namespace_completions;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::support::install_library_package;
use crate::support::install_library_package_files;
use crate::support::install_workspace_package;
use crate::support::install_workspace_package_files;

fn set_namespace(db: &mut OakDatabase, file: File, text: &str) {
    let package = file.package(db).unwrap();
    let namespace = Namespace::parse(text).unwrap();
    package.set_namespace_override(db).to(Some(namespace));
}

fn items(
    db: &OakDatabase,
    package: &str,
    visibility: NamespaceVisibility,
) -> Vec<(String, BindingKind)> {
    namespace_completions(db, package, visibility)
        .unwrap()
        .items
        .into_iter()
        .map(|item| (item.name, item.kind))
        .collect()
}

fn item(name: &str, kind: BindingKind) -> (String, BindingKind) {
    (name.to_string(), kind)
}

const SOURCE: &str = "\
exported <- function() 1
internal <- function() 2
value <- 1
.hidden <- function() 3
";

#[test]
fn test_exports_versus_all_bindings() {
    let mut db = OakDatabase::new();
    install_library_package(&mut db, "pkg", &["exported", "value"], "a.R", SOURCE);

    assert_eq!(items(&db, "pkg", NamespaceVisibility::Exported), vec![
        item("exported", BindingKind::Function),
        item("value", BindingKind::Unknown),
    ]);
    assert_eq!(items(&db, "pkg", NamespaceVisibility::Internal), vec![
        item(".hidden", BindingKind::Function),
        item("exported", BindingKind::Function),
        item("internal", BindingKind::Function),
        item("value", BindingKind::Unknown),
    ]);
}

#[test]
fn test_library_completions_depend_on_their_sources() {
    let mut db = OakDatabase::new();
    let file = install_library_package(&mut db, "pkg", &["exported"], "a.R", SOURCE);

    let completions = namespace_completions(&db, "pkg", NamespaceVisibility::Exported).unwrap();
    assert_eq!(completions.incomplete_until_packages_settle, vec![file
        .package(&db)
        .unwrap()]);
}

#[test]
fn test_workspace_completions_depend_on_no_sources() {
    let mut db = OakDatabase::new();
    install_workspace_package(&mut db, "pkg", &["exported"], "a.R", SOURCE);

    let completions = namespace_completions(&db, "pkg", NamespaceVisibility::Exported).unwrap();
    assert_eq!(completions.incomplete_until_packages_settle, vec![]);
}

fn set_package_namespace(db: &mut OakDatabase, package: &str, text: &str) {
    let package = db.package_by_name(package).unwrap();
    let namespace = Namespace::parse(text).unwrap();
    package.set_namespace_override(db).to(Some(namespace));
}

#[test]
fn test_reexport_depends_on_its_pending_source() {
    // `source` exports `foo` but its sources haven't been fetched, so the
    // re-export's kind is unknown until they are.
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
    let names = reexport.names(&db, NamespaceVisibility::Exported);
    assert_eq!(names.bindings.get("foo"), Some(BindingKind::Unknown));
    assert_eq!(names.reexport_sources, vec![source]);

    let completions =
        namespace_completions(&db, "reexport", NamespaceVisibility::Exported).unwrap();
    assert_eq!(completions.incomplete_until_packages_settle, vec![
        reexport, source
    ]);
}

#[test]
fn test_reexport_chain_reports_every_hop() {
    let mut db = OakDatabase::new();
    install_library_package(&mut db, "deep", &["foo"], "a.R", "foo <- function() 1\n");
    install_library_package_files(&mut db, "mid", &[], &[]);
    set_package_namespace(&mut db, "mid", "importFrom(deep, foo)\nexport(foo)\n");
    install_workspace_package_files(&mut db, "top", &[], &[]);
    set_package_namespace(&mut db, "top", "importFrom(mid, foo)\nexport(foo)\n");

    let top = db.package_by_name("top").unwrap();
    let mid = db.package_by_name("mid").unwrap();
    let deep = db.package_by_name("deep").unwrap();
    let names = top.names(&db, NamespaceVisibility::Exported);
    assert_eq!(names.bindings.get("foo"), Some(BindingKind::Function));
    assert_eq!(names.reexport_sources, vec![mid, deep]);

    // Only library dependencies need to be reported as potentially incomplete.
    let completions = namespace_completions(&db, "top", NamespaceVisibility::Exported).unwrap();
    assert_eq!(completions.incomplete_until_packages_settle, vec![
        mid, deep
    ]);
}

#[test]
fn test_reexport_cycle_reports_the_other_package() {
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "a", &[], &[]);
    set_package_namespace(&mut db, "a", "importFrom(b, foo)\nexport(foo)\n");
    install_library_package_files(&mut db, "b", &[], &[]);
    set_package_namespace(&mut db, "b", "importFrom(a, foo)\nexport(foo)\n");

    let a = db.package_by_name("a").unwrap();
    let b = db.package_by_name("b").unwrap();
    let names = a.names(&db, NamespaceVisibility::Exported);
    assert_eq!(names.bindings.get("foo"), Some(BindingKind::Unknown));
    assert_eq!(names.reexport_sources, vec![b]);
}

#[test]
fn test_reexported_function_keeps_function_kind() {
    let mut db = OakDatabase::new();
    install_library_package(
        &mut db,
        "tibble",
        &["tibble", "tbl_value"],
        "tibble.R",
        "tibble <- function() 1\ntbl_value <- 1\n",
    );
    let dplyr = install_library_package(
        &mut db,
        "dplyr",
        &[],
        "filter.R",
        "filter <- function() 1\n",
    );
    set_namespace(
        &mut db,
        dplyr,
        "export(filter, tibble, tbl_value, ghost)\nimportFrom(tibble, tibble, tbl_value)\n",
    );

    assert_eq!(items(&db, "dplyr", NamespaceVisibility::Exported), vec![
        item("filter", BindingKind::Function),
        item("ghost", BindingKind::Unknown),
        item("tbl_value", BindingKind::Unknown),
        item("tibble", BindingKind::Function),
    ]);
    assert_eq!(items(&db, "dplyr", NamespaceVisibility::Internal), vec![
        item("filter", BindingKind::Function),
        item("ghost", BindingKind::Unknown),
    ]);
}

#[test]
fn test_exports_before_sources_are_fetched() {
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "pkg", &["foo", "bar"], &[]);

    let expected = vec![
        item("bar", BindingKind::Unknown),
        item("foo", BindingKind::Unknown),
    ];
    assert_eq!(items(&db, "pkg", NamespaceVisibility::Exported), expected);
    assert_eq!(items(&db, "pkg", NamespaceVisibility::Internal), expected);
}

#[test]
fn test_base_exports_every_binding() {
    // `base` has no `NAMESPACE`. Include primitive stubs alongside archive bindings.
    let mut db = OakDatabase::new();
    install_library_package_files(&mut db, "base", &[], &[
        (
            "base.R",
            "paste <- function(...) NULL\n.Machine <- list()\n",
        ),
        (
            "stubs.R",
            "`[` <- .Primitive(\"[\")\nsum <- function(..., na.rm = FALSE) NULL\n",
        ),
    ]);

    let expected = vec![
        item(".Machine", BindingKind::Unknown),
        item("[", BindingKind::Function),
        item("paste", BindingKind::Function),
        item("sum", BindingKind::Function),
    ];
    assert_eq!(items(&db, "base", NamespaceVisibility::Exported), expected);
    assert_eq!(items(&db, "base", NamespaceVisibility::Internal), expected);
}

#[test]
fn test_export_pattern_in_completions_and_resolve() {
    let mut db = OakDatabase::new();
    let file = install_library_package(&mut db, "pkg", &[], "a.R", SOURCE);
    set_namespace(&mut db, file, "exportPattern(\"^[^\\\\.]\")\n");

    assert_eq!(items(&db, "pkg", NamespaceVisibility::Exported), vec![
        item("exported", BindingKind::Function),
        item("internal", BindingKind::Function),
        item("value", BindingKind::Unknown),
    ]);

    let package = file.package(&db).unwrap();
    let resolve = |name: &str| {
        package
            .resolve(&db, Name::new(&db, name), NamespaceVisibility::Exported)
            .len()
    };
    assert_eq!(resolve("internal"), 1);
    assert_eq!(resolve(".hidden"), 0);
}

#[test]
fn test_export_pattern_does_not_reach_imports() {
    let mut db = OakDatabase::new();
    install_library_package(
        &mut db,
        "src",
        &["imported"],
        "a.R",
        "imported <- function() 1\n",
    );
    let file = install_library_package(&mut db, "pkg", &[], "a.R", "own <- 1\n");
    set_namespace(
        &mut db,
        file,
        "exportPattern(\".\")\nimportFrom(src, imported)\n",
    );

    assert_eq!(items(&db, "pkg", NamespaceVisibility::Exported), vec![
        item("own", BindingKind::Unknown)
    ]);
}

#[test]
fn test_export_classes_are_not_listed() {
    let mut db = OakDatabase::new();
    let file = install_library_package(&mut db, "pkg", &[], "a.R", "bar <- function() 1\n");
    set_namespace(&mut db, file, "exportClasses(Foo)\nexport(bar)\n");

    assert_eq!(items(&db, "pkg", NamespaceVisibility::Exported), vec![
        item("bar", BindingKind::Function)
    ]);
}

#[test]
fn test_lazy_data_under_exports_only() {
    let mut db = OakDatabase::new();
    let file = install_library_package(&mut db, "pkg", &["exported"], "a.R", SOURCE);
    let package = file.package(&db).unwrap();
    package.set_lazy_data(&mut db, vec![
        String::from("BJsales.lead"),
        String::from("BJsales"),
        String::from("exported"),
    ]);

    assert_eq!(items(&db, "pkg", NamespaceVisibility::Exported), vec![
        item("BJsales", BindingKind::Unknown),
        item("BJsales.lead", BindingKind::Unknown),
        item("exported", BindingKind::Function),
    ]);
    assert_eq!(items(&db, "pkg", NamespaceVisibility::Internal), vec![
        item(".hidden", BindingKind::Function),
        item("exported", BindingKind::Function),
        item("internal", BindingKind::Function),
        item("value", BindingKind::Unknown),
    ]);

    // An explicit export takes precedence over lazy data with the same name.
    let names = package.names(&db, NamespaceVisibility::Exported);
    assert_eq!(names.lazy_data_only, vec!["BJsales", "BJsales.lead"]);
    let names = package.names(&db, NamespaceVisibility::Internal);
    assert_eq!(names.lazy_data_only, Vec::<String>::new());
}

#[test]
fn test_unknown_package() {
    let db = OakDatabase::new();
    assert_eq!(
        namespace_completions(&db, "nope", NamespaceVisibility::Exported),
        None
    );
    assert_eq!(
        namespace_completions(&db, "nope", NamespaceVisibility::Internal),
        None
    );
}
