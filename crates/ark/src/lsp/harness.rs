use crate::console::console_scopes;

/// Initialize R and return the `ReadConsole` scopes needed to resolve base
/// symbols in diagnostics benchmarks.
pub fn r_session_scopes() -> Vec<Vec<String>> {
    harp::fixtures::r_test_init();
    console_scopes()
}
