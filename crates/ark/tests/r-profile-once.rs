use std::io::Write;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontendRprofile;

// SAFETY:
// Do not write any other tests related to `.Rprofile` in
// this integration test file. We can only start R up once
// per process, so we can only run one `.Rprofile`. Use a
// separate integration test (i.e. separate process) if you
// need to test more details related to `.Rprofile` usage.

/// See https://github.com/posit-dev/positron/issues/4253
#[test]
fn test_r_profile_is_only_run_once() {
    // The trailing `\n` is critical, otherwise R's `source()` silently fails
    let contents = r#"
if (exists("x")) {
  x <- 2
} else {
  x <- 1
}

"#;

    // Write `contents` to a tempfile that we declare to be
    // the `.Rprofile` that R should use
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{contents}").unwrap();

    let path = file.path();
    let path = path.to_str().unwrap();

    unsafe { std::env::set_var("R_PROFILE_USER", path) };

    // Ok, start R. If we've set everything correctly, R should not run
    // the `.Rprofile`, but ark should - i.e. it should run exactly 1 time.
    let frontend = DummyArkFrontendRprofile::lock();

    frontend.send_execute_request("x", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "x");
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 1");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}
