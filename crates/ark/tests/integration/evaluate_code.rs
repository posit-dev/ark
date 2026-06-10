//
// evaluate-code.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//
use amalthea::comm::ui_comm::EvalResult;
use amalthea::comm::ui_comm::UiBackendReply;
use ark_test::DummyArkFrontend;

/// Helper to send an `evaluate_code` RPC to the UI comm and return the reply.
///
/// Builds a JSON payload matching the wire format expected by the shell's
/// `handle_comm_msg`: a `method`/`params` object with an `id` field so the
/// shell treats it as an RPC.
fn evaluate_code(frontend: &DummyArkFrontend, comm_id: &str, code: &str) -> UiBackendReply {
    let data = serde_json::json!({
        "method": "evaluate_code",
        "params": { "code": code },
        "id": "eval-rpc"
    });

    frontend.send_shell_comm_msg(String::from(comm_id), data);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_iopub_comm_msg();
    assert_eq!(reply.comm_id, comm_id);
    frontend.recv_iopub_idle();

    serde_json::from_value::<UiBackendReply>(reply.data).unwrap()
}

#[test]
fn test_evaluate_code_result() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    let reply = evaluate_code(&frontend, &comm_id, "1 + 1");
    assert_eq!(
        reply,
        UiBackendReply::EvaluateCodeReply(EvalResult {
            result: serde_json::Value::from(2.0),
            output: String::from(""),
        })
    );
}

#[test]
fn test_evaluate_code_output_capture() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // cat() prints output and returns invisible NULL
    let reply = evaluate_code(&frontend, &comm_id, "cat('hello\\nworld')");
    assert_eq!(
        reply,
        UiBackendReply::EvaluateCodeReply(EvalResult {
            result: serde_json::Value::Null,
            output: String::from("hello\nworld"),
        })
    );
}

#[test]
fn test_evaluate_code_output_and_value() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // cat() prints "oatmeal" and returns invisible NULL; isTRUE(NULL) is FALSE.
    // The output should be captured and the result should be FALSE.
    let reply = evaluate_code(&frontend, &comm_id, "isTRUE(cat('oatmeal'))");
    assert_eq!(
        reply,
        UiBackendReply::EvaluateCodeReply(EvalResult {
            result: serde_json::Value::from(false),
            output: String::from("oatmeal"),
        })
    );
}

#[test]
fn test_evaluate_code_warning_capture() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    let reply = evaluate_code(&frontend, &comm_id, "{ warning('watch out'); 42 }");
    match &reply {
        UiBackendReply::EvaluateCodeReply(eval) => {
            assert_eq!(eval.result, serde_json::Value::from(42.0));
            assert!(eval.output.contains("watch out"));
        },
        other => panic!("Expected EvaluateCodeReply, got: {other:?}"),
    }
}
