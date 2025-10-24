/*
 * client.rs
 *
 * Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
 *
 */

mod control;
mod dummy_frontend;
mod shell;

use amalthea::assert_no_incoming;
use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::recv_iopub_busy;
use amalthea::recv_iopub_comm_close;
use amalthea::recv_iopub_execute_input;
use amalthea::recv_iopub_execute_result;
use amalthea::recv_iopub_idle;
use amalthea::recv_iopub_stream_stdout;
use amalthea::recv_shell_execute_reply;
use amalthea::recv_stdin_input_request;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use amalthea::wire::comm_close::CommClose;
use amalthea::wire::comm_info_reply::CommInfoTargetName;
use amalthea::wire::comm_info_request::CommInfoRequest;
use amalthea::wire::comm_msg::CommWireMsg;
use amalthea::wire::comm_open::CommOpen;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::status::ExecutionState;
use assert_matches::assert_matches;
use dummy_frontend::DummyAmaltheaFrontend;
use serde_json;

#[test]
fn test_amalthea_kernel_info() {
    let frontend = DummyAmaltheaFrontend::lock();

    // Ask the kernel for the kernel info. This should return an object with the
    // language "Test" defined in our shell handler.
    frontend.send_shell(KernelInfoRequest {});
    recv_iopub_busy!(frontend);

    assert_matches!(
        frontend.recv_shell(),
        Message::KernelInfoReply(reply) => {
            assert_eq!(reply.content.language_info.name, "Test");
            assert_eq!(reply.content.protocol_version, "5.4");
            assert!(reply.content.supported_features.contains(&String::from("iopub_welcome")));
        }
    );

    recv_iopub_idle!(frontend);
}

#[test]
fn test_amalthea_execute_request() {
    let frontend = DummyAmaltheaFrontend::lock();

    let code = "42";
    frontend.send_execute_request(code, Default::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert_eq!(recv_iopub_execute_result!(frontend), "42");
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    recv_iopub_idle!(frontend);
}

#[test]
fn test_amalthea_input_request() {
    let frontend = DummyAmaltheaFrontend::lock();

    let code = "prompt";
    frontend.send_execute_request(code, Default::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    let prompt = recv_stdin_input_request!(frontend);
    assert_eq!(prompt, "Amalthea Echo> ");

    frontend.send_stdin_input_reply(String::from("42"));

    recv_iopub_stream_stdout!(frontend, "42");
    assert_eq!(recv_iopub_execute_result!(frontend), "prompt");

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    recv_iopub_idle!(frontend);
}

#[test]
fn test_amalthea_heartbeat() {
    let frontend = DummyAmaltheaFrontend::lock();

    frontend.send_heartbeat(zmq::Message::from("Heartbeat"));
    assert_eq!(frontend.recv_heartbeat(), zmq::Message::from("Heartbeat"));
}

#[test]
fn test_amalthea_comms() {
    let frontend = DummyAmaltheaFrontend::lock();

    let comm_id = "A3A6D0EA-1443-4F70-B059-F423E445B8D6";

    frontend.send_shell(CommOpen {
        comm_id: comm_id.to_string(),
        target_name: "unknown".to_string(),
        data: serde_json::Value::Null,
    });

    recv_iopub_busy!(frontend);
    assert_eq!(recv_iopub_comm_close!(frontend), comm_id.to_string());
    recv_iopub_idle!(frontend);

    frontend.send_shell(CommOpen {
        comm_id: comm_id.to_string(),
        target_name: "variables".to_string(),
        data: serde_json::Value::Null,
    });

    // Absorb the IOPub messages that the kernel sends back during the
    // processing of the above `CommOpen` request
    recv_iopub_busy!(frontend);
    recv_iopub_idle!(frontend);
    assert_no_incoming!(frontend);

    frontend.send_shell(CommInfoRequest {
        target_name: "".to_string(),
    });
    recv_iopub_busy!(frontend);

    assert_matches!(frontend.recv_shell(), Message::CommInfoReply(request) => {
        // Ensure the comm we just opened is in the list of comms
        let comms = request.content.comms;
        assert!(comms.contains_key(comm_id));
    });

    recv_iopub_idle!(frontend);
    assert_no_incoming!(frontend);

    // Test requesting comm info and filtering by target name. We should get
    // back an empty list of comms, since we haven't opened any comms with
    // the target name "i-think-not".
    frontend.send_shell(CommInfoRequest {
        target_name: "i-think-not".to_string(),
    });
    recv_iopub_busy!(frontend);

    assert_matches!(frontend.recv_shell(), Message::CommInfoReply(request) => {
        let comms = request.content.comms;
        assert!(comms.is_empty());
    });

    recv_iopub_idle!(frontend);
    assert_no_incoming!(frontend);

    let comm_req_id = frontend.send_shell(CommWireMsg {
        comm_id: comm_id.to_string(),
        // Include `id` field to signal this is a request
        data: serde_json::json!({ "id": "foo" }),
    });

    recv_iopub_busy!(frontend);

    let mut got_idle = false;
    let mut got_reply = false;

    // This runs in a loop because the ordering of the Idle status and the reply
    // is undetermined.
    loop {
        match frontend.recv_iopub() {
            Message::CommMsg(msg) => {
                if got_reply {
                    panic!("Received multiple comm messages");
                }
                // Ensure that the comm ID in the message matches the comm ID we
                // sent
                assert_eq!(msg.content.comm_id, comm_id);

                // Ensure that the parent message ID in the message exists and
                // matches the message ID of the comm message we sent; this is
                // how RPC responses are aligned with requests
                assert_eq!(msg.parent_header.unwrap().msg_id, comm_req_id);

                got_reply = true;
            },
            Message::Status(msg) => {
                if got_idle {
                    panic!("Received multiple idle messages");
                }
                assert_eq!(msg.content.execution_state, ExecutionState::Idle);
                got_idle = true;
            },
            msg => {
                panic!("Unexpected IOPub message: {msg:?}");
            },
        }

        if got_idle && got_reply {
            break;
        }
    }

    // Test closing the comm we just opened
    frontend.send_shell(CommClose {
        comm_id: comm_id.to_string(),
    });

    recv_iopub_busy!(frontend);
    recv_iopub_idle!(frontend);

    // Test to see if the comm is still in the list of comms after closing it
    // (it should not be)
    frontend.send_shell(CommInfoRequest {
        target_name: "variables".to_string(),
    });
    recv_iopub_busy!(frontend);

    assert_matches!(frontend.recv_shell(), Message::CommInfoReply(request) => {
        // Ensure the comm we just closed not present in the list of comms
        let comms = request.content.comms;
        assert!(!comms.contains_key(comm_id));
    });

    recv_iopub_idle!(frontend);
}

#[test]
fn test_amalthea_comm_open_from_kernel() {
    let frontend = DummyAmaltheaFrontend::lock();

    // Now test opening a comm from the kernel side

    let test_comm_id = String::from("test_comm_id_84e7fe");
    let test_comm_name = String::from("test_target");
    let test_comm = CommSocket::new(
        CommInitiator::BackEnd,
        test_comm_id.clone(),
        test_comm_name.clone(),
    );

    frontend
        .comm_manager_tx
        .send(CommManagerEvent::Opened(
            test_comm.clone(),
            serde_json::Value::Null,
        ))
        .unwrap();

    // Wait for the comm open message to be received by the frontend. We should get
    // a CommOpen message on the IOPub channel notifying the frontend that the new comm
    // has been opened.
    assert_matches!(frontend.recv_iopub(), Message::CommOpen(msg) => {
        assert_eq!(msg.content.comm_id, test_comm_id);
    });

    // Query the kernel to see if the comm we just opened is in the list of
    // comms. It's similar to the test done above for opening a comm from the
    // frontend, but this time we're testing the other direction, to ensure that
    // the kernel is correctly tracking the list of comms regardless of where
    // they originated.
    frontend.send_shell(CommInfoRequest {
        target_name: test_comm_name.clone(),
    });

    recv_iopub_busy!(frontend);

    assert_matches!(frontend.recv_shell(), Message::CommInfoReply(request) => {
        // Ensure the comm we just opened is in the list of comms
        let comms = request.content.comms;
        assert!(comms.contains_key(&test_comm_id));

        // Ensure the comm we just opened has the correct target name
        let comm = comms.get(&test_comm_id).unwrap();
        let target = serde_json::from_value::<CommInfoTargetName>(comm.clone()).unwrap();
        assert!(target.target_name == test_comm_name)
    });

    recv_iopub_idle!(frontend);

    // Now send a message from the backend to the frontend using the comm we just
    // created.
    test_comm
        .outgoing_tx
        .send(CommMsg::Data(serde_json::Value::Null))
        .unwrap();

    assert_matches!(frontend.recv_iopub(), Message::CommMsg(msg) => {
        assert_eq!(msg.content.comm_id, test_comm_id);
    });

    // Close the test comm from the backend side
    test_comm.outgoing_tx.send(CommMsg::Close).unwrap();

    // Ensure that the frontend is notified
    assert_matches!(frontend.recv_iopub(), Message::CommClose(msg) => {
        assert_eq!(msg.content.comm_id, test_comm_id);
    });
}
