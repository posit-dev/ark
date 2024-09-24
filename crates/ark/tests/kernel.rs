use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use ark::test::DummyArkFrontend;
use serde_json::Value;
use stdext::assert_match;

#[test]
fn test_kernel_info() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_shell(KernelInfoRequest {});

    assert_match!(frontend.receive_shell(), Message::KernelInfoReply(reply) => {
        assert_eq!(reply.content.language_info.name, "R");
    });

    frontend.receive_iopub_busy();
    frontend.receive_iopub_idle();
}

#[test]
fn test_execute_request() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("42");
    frontend.receive_iopub_busy();

    // Input rebroadcast
    assert_match!(frontend.receive_iopub(), Message::ExecuteInput(msg) => {
        assert_eq!(msg.content.code, "42");
    });

    assert_match!(frontend.receive_iopub(), Message::ExecuteResult(msg) => {
        assert_match!(msg.content.data, Value::Object(map) => {
            assert_eq!(map["text/plain"], serde_json::to_value("[1] 42").unwrap());
        })
    });

    frontend.receive_iopub_idle();

    assert_match!(frontend.receive_shell(), Message::ExecuteReply(msg) => {
        assert_eq!(msg.content.status, Status::Ok);
    });
}
