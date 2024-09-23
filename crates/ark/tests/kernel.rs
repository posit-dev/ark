use amalthea::test::dummy_frontend::DummyFrontend;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use ark::interface::SessionMode;
use serde_json::Value;
use stdext::assert_match;
use stdext::spawn;

fn spawn_r() -> DummyFrontend {
    let frontend = DummyFrontend::new();
    let connection_file = frontend.get_connection_file();

    spawn!("dummy_kernel", || {
        ark::start::start_kernel(connection_file, vec![], None, SessionMode::Console, false);
    });

    // Can we do better?
    log::info!("Waiting 500ms for kernel startup to complete");
    std::thread::sleep(std::time::Duration::from_millis(500));

    frontend.complete_intialization();

    frontend
}

#[test]
fn test_kernel() {
    let frontend = spawn_r();

    // --- Kernel info
    frontend.send_shell(KernelInfoRequest {});

    assert_match!(frontend.receive_shell(), Message::KernelInfoReply(reply) => {
        assert_eq!(reply.content.language_info.name, "R");
    });

    frontend.receive_iopub_busy();
    frontend.receive_iopub_idle();

    // --- Execute request
    frontend.send_shell(ExecuteRequest {
        code: "42".to_string(),
        silent: false,
        store_history: true,
        user_expressions: serde_json::Value::Null,
        allow_stdin: false,
        stop_on_error: false,
    });

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
}
