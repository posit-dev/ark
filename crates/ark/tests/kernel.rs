use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use ark::fixtures::DummyArkFrontend;
use stdext::assert_match;

#[test]
fn test_kernel_info() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_shell(KernelInfoRequest {});

    assert_match!(frontend.recv_shell(), Message::KernelInfoReply(reply) => {
        assert_eq!(reply.content.language_info.name, "R");
        assert_eq!(reply.content.language_info.pygments_lexer, None);
        assert_eq!(reply.content.language_info.codemirror_mode, None);
        assert_eq!(reply.content.language_info.nbconvert_exporter, None);
    });

    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

#[test]
fn test_env_vars() {
    // These environment variables are set by R's shell script frontend.
    // We set these in Ark as well.
    let frontend = DummyArkFrontend::lock();

    let code = "stopifnot(
            identical(Sys.getenv('R_SHARE_DIR'), R.home('share')),
            identical(Sys.getenv('R_INCLUDE_DIR'), R.home('include')),
            identical(Sys.getenv('R_DOC_DIR'), R.home('doc'))
        )";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}
