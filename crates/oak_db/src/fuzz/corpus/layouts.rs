//! Named scenarios for convention-driven project layouts.

use super::package;
use super::program;
use super::query;
use super::replace;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;

/// testthat loads the package collation and `helper*.R` files before each test file.
pub(super) fn testthat_test_sees_helpers_and_package() -> Scenario {
    let initial = package("mypkg", &["base", "testthat"], vec![
        ("R/a.R", program(vec![function_def("pkg_fn", vec![])])),
        (
            "tests/testthat/helper-b.R",
            program(vec![function_def("helper_fn", vec![])]),
        ),
        ("tests/testthat/test-c.R", program(vec![binding("val_c")])),
    ]);
    Scenario::cold(initial, Query::Diagnostics(FileId(2)), vec![])
}

/// Helper edits update a test file's view without changing the package collation.
///
/// The runner discards edit results, so the trailing `Query::Resolve` forces
/// recomputation of the test file's view.
pub(super) fn testthat_helper_edit_changes_the_test_view() -> Scenario {
    let scenario = testthat_test_sees_helpers_and_package();
    let ops = vec![
        replace(FileId(1), program(vec![binding("helper_val")])),
        query(Query::Resolve(FileId(2), "helper_val".to_string())),
    ];
    Scenario::cold(scenario.initial, Query::Diagnostics(FileId(2)), ops)
}
