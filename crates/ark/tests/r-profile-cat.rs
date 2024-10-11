use std::io::Write;

use ark::fixtures::DummyArkFrontendRprofile;

// SAFETY:
// Do not write any other tests related to `.Rprofile` in
// this integration test file. We can only start R up once
// per process, so we can only run one `.Rprofile`. Use a
// separate integration test (i.e. separate process) if you
// need to test more details related to `.Rprofile` usage.

/// See https://github.com/posit-dev/positron/issues/4973
#[test]
fn test_r_profile_can_cat() {
    let message = "hi from rprofile";

    // The `\n` is critical, otherwise R's `source()` silently fails
    let contents = format!("cat('{message}')\n");

    // Write `contents` to a tempfile that we declare to be
    // the `.Rprofile` that R should use
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{contents}").unwrap();

    let path = file.path();
    let path = path.to_str().unwrap();

    unsafe { std::env::set_var("R_PROFILE_USER", path) };

    // Ok, load up R now. It should `cat()` the `message` over iopub.
    let frontend = DummyArkFrontendRprofile::lock();

    frontend.recv_iopub_stream_stdout(message)
}
