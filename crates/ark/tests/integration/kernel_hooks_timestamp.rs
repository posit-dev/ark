use ark_test::DummyArkFrontend;

#[test]
fn test_timestamp() {
    // This test is mostly to ensure that our `utils::timestamp()` override on Windows
    // avoids a crash. So as long as this test passes on all platforms, we are happy!
    // https://github.com/posit-dev/positron/issues/13261

    let frontend = DummyArkFrontend::lock();

    // By default it `cat()`s to stdout and returns invisibly. We want the actual
    // string timestamp.
    let code = "withVisible(utils::timestamp(quiet = TRUE))$value";

    // The actual timestamp changes, but the start and end fragments should be there
    frontend.execute_request(code, |result| {
        assert!(result.contains("##------"));
        assert!(result.contains("------##"));
    });
}
