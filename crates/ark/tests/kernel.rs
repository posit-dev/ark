use amalthea::wire::jupyter_message::Message;
use amalthea::wire::jupyter_message::Status;
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
fn test_execute_request() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("42");
    frontend.recv_iopub_busy();

    assert_eq!(frontend.recv_iopub_execute_input().code, "42");
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 42");

    frontend.recv_iopub_idle();

    let reply = frontend.recv_shell_execute_reply();
    assert_eq!(reply.status, Status::Ok);
}
