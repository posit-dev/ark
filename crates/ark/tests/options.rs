use std::io::Write;

use ark_test::DummyArkFrontendRprofile;

// These tests verify that options set in `.Rprofile` interact correctly with
// `initialize_options()` during startup. Each test needs its own process
// because `DummyArkFrontendRprofile` can only be locked once, which is
// handled by nextest.

#[test]
fn test_override_option_replaces_user_value() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    writeln!(file, "options(max.print = 500)").unwrap();
    unsafe { std::env::set_var("R_PROFILE_USER", file.path()) };

    let frontend = DummyArkFrontendRprofile::lock();

    frontend.execute_request("getOption('max.print')", |result| {
        assert_eq!(result, "[1] 1000");
    });
}

#[test]
fn test_protected_override_option_keeps_user_value() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        file,
        "options(max.print = 500, ark.protected_options = 'max.print')"
    )
    .unwrap();
    unsafe { std::env::set_var("R_PROFILE_USER", file.path()) };

    let frontend = DummyArkFrontendRprofile::lock();

    frontend.execute_request("getOption('max.print')", |result| {
        assert_eq!(result, "[1] 500");
    });
}

#[test]
fn test_default_option_keeps_user_value() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    writeln!(file, "options(help_type = 'text')").unwrap();
    unsafe { std::env::set_var("R_PROFILE_USER", file.path()) };

    let frontend = DummyArkFrontendRprofile::lock();

    frontend.execute_request("getOption('help_type')", |result| {
        assert_eq!(result, "[1] \"text\"");
    });
}

#[test]
fn test_default_option_sets_when_null() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    writeln!(file).unwrap();
    unsafe { std::env::set_var("R_PROFILE_USER", file.path()) };

    let frontend = DummyArkFrontendRprofile::lock();

    frontend.execute_request("getOption('help_type')", |result| {
        assert_eq!(result, "[1] \"html\"");
    });
}

#[test]
fn test_protected_default_option_stays_null() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    writeln!(file, "options(ark.protected_options = 'help_type')").unwrap();
    unsafe { std::env::set_var("R_PROFILE_USER", file.path()) };

    let frontend = DummyArkFrontendRprofile::lock();

    frontend.execute_request("is.null(getOption('help_type'))", |result| {
        assert_eq!(result, "[1] TRUE");
    });
}
