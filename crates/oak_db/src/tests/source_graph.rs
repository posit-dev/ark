use std::path::PathBuf;

use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::File;
use crate::Name;
use crate::Package;
use crate::PackageOrigin;
use crate::Script;
use crate::SourceNode;

fn name<'db>(db: &'db TestDb, text: &str) -> Name<'db> {
    Name::new(db, text)
}

fn workspace_origin(name: &str) -> PackageOrigin {
    PackageOrigin::Workspace {
        root: PathBuf::from(format!("/workspace/{name}")),
    }
}

fn installed_origin(_name: &str) -> PackageOrigin {
    PackageOrigin::Installed
}

fn make_package(db: &TestDb, name: &str, kind: PackageOrigin) -> Package {
    Package::new(db, name.to_string(), kind, Namespace::default(), Vec::new())
}

fn make_script(db: &TestDb, name: &str) -> Script {
    let file = File::new(db, file_url(name), String::new());
    Script::new(db, file)
}

#[test]
fn package_by_name_finds_workspace_package() {
    let mut db = TestDb::new();
    let pkg = make_package(&db, "rlang", workspace_origin("rlang"));
    let source_graph = db.source_graph();
    source_graph.set_workspace_packages(&mut db).to(vec![pkg]);

    assert_eq!(
        source_graph.package_by_name(&db, name(&db, "rlang")),
        Some(pkg)
    );
}

#[test]
fn package_by_name_falls_back_to_installed() {
    let mut db = TestDb::new();
    let pkg = make_package(&db, "dplyr", installed_origin("dplyr"));
    let source_graph = db.source_graph();
    source_graph.set_installed_packages(&mut db).to(vec![pkg]);

    assert_eq!(
        source_graph.package_by_name(&db, name(&db, "dplyr")),
        Some(pkg)
    );
}

#[test]
fn package_by_name_workspace_shadows_installed() {
    let mut db = TestDb::new();
    let workspace_pkg = make_package(&db, "rlang", workspace_origin("rlang"));
    let installed_pkg = make_package(&db, "rlang", installed_origin("rlang"));
    let source_graph = db.source_graph();
    source_graph
        .set_workspace_packages(&mut db)
        .to(vec![workspace_pkg]);
    source_graph
        .set_installed_packages(&mut db)
        .to(vec![installed_pkg]);

    assert_eq!(
        source_graph.package_by_name(&db, name(&db, "rlang")),
        Some(workspace_pkg),
    );
}

#[test]
fn package_by_name_returns_none_when_absent() {
    let db = TestDb::new();
    assert_eq!(
        db.source_graph().package_by_name(&db, name(&db, "ggplot2")),
        None,
    );
}

#[test]
fn script_by_url_finds_registered_script() {
    let mut db = TestDb::new();
    let script = make_script(&db, "analysis.R");
    let source_graph = db.source_graph();
    source_graph.set_scripts(&mut db).to(vec![script]);

    assert_eq!(
        source_graph.script_by_url(&db, &file_url("analysis.R")),
        Some(script),
    );
}

#[test]
fn script_by_url_returns_none_for_unknown_url() {
    let db = TestDb::new();
    assert_eq!(
        db.source_graph().script_by_url(&db, &file_url("missing.R")),
        None,
    );
}

#[test]
fn source_node_round_trips_through_a_tracked_query() {
    // SourceNode is a plain enum over Salsa input ids; this exercises
    // it as a tracked-query return type, confirming the auto-derived
    // Update / equality machinery works.
    #[salsa::tracked]
    fn first_node(db: &dyn Db) -> Option<SourceNode> {
        if let Some(&script) = db.source_graph().scripts(db).first() {
            return Some(SourceNode::Script(script));
        }
        if let Some(&package) = db.source_graph().workspace_packages(db).first() {
            return Some(SourceNode::Package(package));
        }
        None
    }

    let mut db = TestDb::new();
    assert_eq!(first_node(&db), None);

    let script = make_script(&db, "a.R");
    let source_graph = db.source_graph();
    source_graph.set_scripts(&mut db).to(vec![script]);
    assert_eq!(first_node(&db), Some(SourceNode::Script(script)));

    source_graph.set_scripts(&mut db).to(vec![]);
    let package = make_package(&db, "rlang", workspace_origin("rlang"));
    source_graph
        .set_workspace_packages(&mut db)
        .to(vec![package]);
    assert_eq!(first_node(&db), Some(SourceNode::Package(package)));
}
