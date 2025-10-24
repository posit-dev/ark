use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::recv_iopub_busy;
use amalthea::recv_iopub_execute_input;
use amalthea::recv_iopub_execute_result;
use amalthea::recv_iopub_idle;
use amalthea::recv_shell_execute_reply;
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
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(
        recv_iopub_execute_result!(frontend),
        format!("[1] \"{value}\"")
    );

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count)
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
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(
        recv_iopub_execute_result!(frontend),
        format!("[1] \"{value}\"")
    );

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count)
}

fn set_var(key: &str, value: &str, frontend: &DummyArkFrontend) {
    let code = format!("Sys.setenv({key} = \"{value}\")");
    frontend.send_execute_request(code.as_str(), ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count)
}

fn has_rstudioapi(frontend: &DummyArkFrontend) -> bool {
    let code = ".ps.is_installed('rstudioapi')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    let result = recv_iopub_execute_result!(frontend);

    let out = if result == "[1] TRUE" {
        true
    } else if result == "[1] FALSE" {
        false
    } else {
        panic!("Expected `TRUE` or `FALSE`, got '{result}'.");
    };

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    out
}

fn report_skipped(f: &str) {
    println!("Skipping `{f}()`. rstudioapi is not installed.");
}
