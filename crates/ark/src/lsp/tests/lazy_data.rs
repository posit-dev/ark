//! Tests for selecting installed packages with lazy data indexes and
//! supplying their object names through the sidecar job.

use std::sync::Arc;

use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::SourceDb;
use oak_scan::DbScan;
use oak_sidecar::primitives::PrimitivesCache;

use super::sidecar_handler::TestSidecarHandler;
use super::utils::initialize;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::DescriptionWriter;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sidecar::OakSidecarHandler;
use crate::lsp::sidecar::SidecarHandler;
use crate::lsp::sidecar::SidecarRequest;
use crate::lsp::sidecar::SidecarScheduler;
use crate::lsp::sources::SourceScheduler;

#[test]
fn test_sidecar_reads_lazy_data_like_read_rds() {
    let r_home = harp::command::r_home_setup().unwrap();
    let r = harp::command::r_executable(&r_home).unwrap();

    let output = oak_r_process::run_text(
        &r,
        r#"
path <- system.file("data", "Rdata.rds", package = "datasets", mustWork = TRUE)
names <- unlist(readRDS(path), use.names = FALSE)
writeLines(sort(unique(names), method = "radix"))
"#,
        &[],
        &[],
    )
    .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let expected: Vec<&str> = stdout.lines().collect();

    // `BJsales` creates two objects, so the dataset name alone is not enough.
    assert!(expected.contains(&"BJsales"));
    assert!(expected.contains(&"BJsales.lead"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[r_home.join("library")]);
    let datasets = db.package_by_name("datasets").unwrap();
    let utils = db.package_by_name("utils").unwrap();

    let request = SidecarRequest::from_db(&db);
    let passed: Vec<Package> = request
        .packages()
        .iter()
        .map(|package| package.package())
        .collect();
    assert!(passed.contains(&datasets));
    assert!(!passed.contains(&utils));

    let cache = tempfile::tempdir().unwrap();
    let primitives = PrimitivesCache::open_in(cache.path().to_path_buf(), r.clone()).unwrap();
    let handler = OakSidecarHandler::with_primitives(r, primitives);
    let response = handler.handle(&request);

    let facts = response
        .packages
        .into_iter()
        .find(|facts| facts.package == datasets)
        .unwrap();
    datasets.set_lazy_data(&mut db, facts.lazy_data.unwrap());
    assert_eq!(datasets.lazy_data(&db), expected);
}

#[tokio::test]
async fn test_packages_without_rdata_are_not_passed() {
    let _aux = init_aux_for_test();

    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("base")
        .version("4.5.2")
        .built("dummy")
        .field("Priority", "base")
        .write_installed(&lib.path().join("base"));
    DescriptionWriter::new()
        .package("nodata")
        .version("0.0.0")
        .built("dummy")
        .write_installed(&lib.path().join("nodata"));
    let withdata = lib.path().join("withdata");
    DescriptionWriter::new()
        .package("withdata")
        .version("0.0.0")
        .built("dummy")
        .write_installed(&withdata);
    std::fs::create_dir_all(withdata.join("data")).unwrap();
    std::fs::write(withdata.join("data").join("Rdata.rds"), "").unwrap();

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let base = db.package_by_name("base").unwrap();
    let nodata = db.package_by_name("nodata").unwrap();
    let withdata = db.package_by_name("withdata").unwrap();

    let sidecar = Arc::new(TestSidecarHandler::new(Vec::new()).with_lazy_data(withdata, &["obj"]));
    let mut scheduler = SourceScheduler::new(None);
    scheduler.config_arrived();
    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db),
        LspState::new(tokio::sync::mpsc::unbounded_channel().0)
            .with_source_scheduler(scheduler)
            .with_sidecar_scheduler(SidecarScheduler::new(Some(sidecar.clone()))),
    );

    let workspace = tempfile::tempdir().unwrap();
    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    let calls = sidecar.calls().lock().unwrap();
    assert_eq!(calls.len(), 1);
    let mut passed: Vec<(Package, bool, bool)> = calls[0]
        .packages()
        .iter()
        .map(|package| {
            (
                package.package(),
                package.has_primitives(),
                package.has_lazy_data(),
            )
        })
        .collect();
    passed.sort_by_key(|(package, _, _)| package.name(state.world().db()).clone());
    assert_eq!(passed, vec![(base, true, false), (withdata, false, true)]);

    let db = state.world().db();
    assert!(base.lazy_data(db).is_empty());
    assert!(nodata.lazy_data(db).is_empty());
    assert_eq!(withdata.lazy_data(db), ["obj"]);
}
