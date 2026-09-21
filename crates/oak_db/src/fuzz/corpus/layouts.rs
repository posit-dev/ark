//! Named scenarios for convention-driven project layouts.

use super::package;
use super::program;
use super::query;
use super::replace;
use super::scripts;
use crate::fuzz::build::binding;
use crate::fuzz::build::call;
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

/// `shiny::loadSupport()` loads `global.R` and `R/` autoload siblings for a Shiny entry file.
pub(super) fn shiny_entry_sees_global_and_r_files() -> Scenario {
    let initial = scripts(vec![
        ("app.R", program(vec![call("shinyApp")])),
        ("global.R", program(vec![function_def("global_fn", vec![])])),
        ("R/a.R", program(vec![function_def("r_fn", vec![])])),
    ]);
    Scenario::cold(initial, Query::Diagnostics(FileId(0)), vec![])
}

/// Removing `app.R`'s marker makes it and its `R/` siblings standalone, so neither uses `global.R`.
///
/// The runner discards edit results, so `Query::Resolve` recomputes both views.
pub(super) fn shiny_marker_removed_stops_autoload() -> Scenario {
    let scenario = shiny_entry_sees_global_and_r_files();
    let ops = vec![
        replace(FileId(0), program(vec![binding("not_an_app")])),
        query(Query::Resolve(FileId(0), "r_fn".to_string())),
        query(Query::Resolve(FileId(2), "global_fn".to_string())),
    ];
    Scenario::cold(scenario.initial, Query::Diagnostics(FileId(0)), ops)
}

/// `_disable_autoload.R` is presence-driven: it unclassifies `R/` siblings
/// without affecting `app.R` or its view of `global.R`.
pub(super) fn shiny_disabled_autoload_drops_the_r_sibling() -> Scenario {
    let initial = scripts(vec![
        ("app.R", program(vec![call("shinyApp")])),
        ("global.R", program(vec![function_def("global_fn", vec![])])),
        ("R/_disable_autoload.R", program(vec![binding("marker")])),
        ("R/a.R", program(vec![function_def("r_fn", vec![])])),
    ]);
    Scenario::cold(initial, Query::Diagnostics(FileId(0)), vec![])
}

/// `in_r_directory()` takes precedence over entry detection, so `R/app.R`
/// remains an autoload member despite its `shinyApp()` marker.
pub(super) fn shiny_nested_app_file_joins_the_enclosing_app() -> Scenario {
    let initial = scripts(vec![
        ("app.R", program(vec![call("shinyApp")])),
        ("global.R", program(vec![function_def("global_fn", vec![])])),
        (
            "R/app.R",
            program(vec![call("shinyApp"), function_def("nested_fn", vec![])]),
        ),
    ]);
    Scenario::cold(initial, Query::Diagnostics(FileId(2)), vec![])
}
