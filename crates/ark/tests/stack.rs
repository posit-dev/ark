use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::recv_iopub_busy;
use amalthea::recv_iopub_execute_input;
use amalthea::recv_iopub_execute_result;
use amalthea::recv_iopub_idle;
use amalthea::recv_shell_execute_reply;
use ark::fixtures::DummyArkFrontend;

// These tests assert that we've correctly turned off the `R_StackLimit` check during integration
// tests that use the `DummyArkFrontend`. It is turned off using `stdext::IS_TESTING` in the
// platform specific `interface.rs`.

#[test]
fn test_stack_info_size() {
    let frontend = DummyArkFrontend::lock();

    let code = "Cstack_info()[['size']]";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] NA");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count)
}

#[test]
fn test_stack_info_current() {
    let frontend = DummyArkFrontend::lock();

    let code = "Cstack_info()[['current']]";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] NA");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count)
}
