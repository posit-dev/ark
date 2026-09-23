use aether_path::FilePath;
use oak_db::OakDatabase;
use url::Url;

use crate::lsp::harness::LspHarness;
use crate::r_task;

/// A snapshot must serve documents opened by the live state, as
/// `DiagnosticsState::refresh_all()` requires when it sends work to a worker.
#[test]
fn test_diagnostics_published_through_refresh_snapshot() {
    let url = Url::parse("file:///test.R").unwrap();
    let path = FilePath::from_url(&url);

    // `r_task()` must enclose the harness because Salsa ingredients cannot cross
    // the task's thread boundary.
    let diagnostics = r_task(|| {
        let mut harness = LspHarness::new(OakDatabase::new());
        harness
            .prepare_document(&path, String::from("foo"))
            .unwrap();

        let snapshot = harness.snapshot();
        harness.diagnose(&path, snapshot).unwrap()
    });

    assert!(!diagnostics.is_empty());
}
