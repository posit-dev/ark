use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontend;

// These tests assert that we've correctly turned off the `R_StackLimit` check during integration
// tests that use the `DummyArkFrontend`. It is turned off using `stdext::IS_TESTING` in the
// platform specific `interface.rs`.

#[test]
fn test_stack_info_size() {
    let frontend = DummyArkFrontend::lock();

    let code = "Cstack_info()[['size']]";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] NA");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

#[test]
fn test_stack_info_current() {
    let frontend = DummyArkFrontend::lock();

    let code = "Cstack_info()[['current']]";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] NA");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}
