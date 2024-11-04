use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontend;

#[test]
fn test_get_version() {
    let frontend = DummyArkFrontend::lock();

    if !has_rstudioapi(&frontend) {
        report_skipped("test_get_version");
        return;
    }

    let value = "1.0.0";
    // Can't directly talk to R, need an `r_task()` that can be used alongside
    // the `frontend`. See https://github.com/posit-dev/ark/issues/609.
    // harp::envvar::set_var("POSITRON_VERSION", value);
    set_var("POSITRON_VERSION", value, &frontend);

    let code = "as.character(rstudioapi::getVersion())";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(
        frontend.recv_iopub_execute_result(),
        format!("[1] \"{value}\"")
    );

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

#[test]
fn test_get_mode() {
    let frontend = DummyArkFrontend::lock();

    if !has_rstudioapi(&frontend) {
        report_skipped("test_get_mode");
        return;
    }

    let value = "desktop";
    // Can't directly talk to R, need an `r_task()` that can be used alongside
    // the `frontend`. See https://github.com/posit-dev/ark/issues/609.
    // harp::envvar::set_var("POSITRON_MODE", value);
    set_var("POSITRON_MODE", value, &frontend);

    let code = "rstudioapi::getMode()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(
        frontend.recv_iopub_execute_result(),
        format!("[1] \"{value}\"")
    );

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

fn set_var(key: &str, value: &str, frontend: &DummyArkFrontend) {
    let code = format!("Sys.setenv({key} = \"{value}\")");
    frontend.send_execute_request(code.as_str(), ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

fn has_rstudioapi(frontend: &DummyArkFrontend) -> bool {
    let code = ".ps.is_installed('rstudioapi')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let result = frontend.recv_iopub_execute_result();

    let out = if result == "[1] TRUE" {
        true
    } else if result == "[1] FALSE" {
        false
    } else {
        panic!("Expected `TRUE` or `FALSE`, got '{result}'.");
    };

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    out
}

fn report_skipped(f: &str) {
    println!("Skipping `{f}()`. rstudioapi is not installed.");
}
