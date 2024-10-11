/*
 * client.rs
 *
 * Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
 *
 */

mod control;
mod dummy_frontend;
mod shell;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
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
use dummy_frontend::DummyAmaltheaFrontend;
use serde_json;

#[test]
fn test_kernel() {
    let mut frontend = DummyAmaltheaFrontend::lock();

    // Ask the kernel for the kernel info. This should return an object with the
    // language "Test" defined in our shell handler.
    log::info!("Requesting kernel information");
    frontend.send_shell(KernelInfoRequest {});

    log::info!("Waiting for kernel info reply");
    let reply = frontend.recv_shell();
    match reply {
        Message::KernelInfoReply(reply) => {
            log::info!("Kernel info received: {:?}", reply);
            assert_eq!(reply.content.language_info.name, "Test");
        },
        _ => {
            panic!("Unexpected message received: {:?}", reply);
        },
    }
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
    frontend.assert_no_incoming();

    // Ask the kernel to execute some code
    log::info!("Requesting execution of code '42'");

    let code = "42";
    frontend.send_execute_request(code, Default::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert_eq!(frontend.recv_iopub_execute_result(), "42");
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.recv_iopub_idle();
    frontend.assert_no_incoming();

    // Test nested input request
    log::info!("Sending request to generate an input prompt");

    let code = "prompt";
    frontend.send_execute_request(code, Default::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, "Amalthea Echo> ");

    frontend.send_stdin_input_reply(String::from("42"));

    frontend.recv_iopub_stream_stdout("42");
    assert_eq!(frontend.recv_iopub_execute_result(), "prompt");

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.recv_iopub_idle();
    frontend.assert_no_incoming();

    // Test the heartbeat
    frontend.send_heartbeat(zmq::Message::from("Heartbeat"));
    assert_eq!(frontend.recv_heartbeat(), zmq::Message::from("Heartbeat"));

    // Test the comms
    log::info!("Sending comm open request to the kernel");
    let comm_id = "A3A6D0EA-1443-4F70-B059-F423E445B8D6";

    frontend.send_shell(CommOpen {
        comm_id: comm_id.to_string(),
        target_name: "unknown".to_string(),
        data: serde_json::Value::Null,
    });

    frontend.recv_iopub_busy();
    assert_eq!(frontend.recv_iopub_comm_close(), comm_id.to_string());
    frontend.recv_iopub_idle();

    frontend.send_shell(CommOpen {
        comm_id: comm_id.to_string(),
        target_name: "variables".to_string(),
        data: serde_json::Value::Null,
    });

    // Absorb the IOPub messages that the kernel sends back during the
    // processing of the above `CommOpen` request
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
    frontend.assert_no_incoming();

    log::info!("Requesting comm info from the kernel (to test opening from the frontend)");
    frontend.send_shell(CommInfoRequest {
        target_name: "".to_string(),
    });
    frontend.recv_iopub_busy();

    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            log::info!("Got comm info: {:?}", request);
            // Ensure the comm we just opened is in the list of comms
            let comms = request.content.comms;
            assert!(comms.contains_key(comm_id));
        },
        _ => {
            panic!(
                "Unexpected message received (expected comm info): {:?}",
                reply
            );
        },
    }

    frontend.recv_iopub_idle();
    frontend.assert_no_incoming();

    // Test requesting comm info and filtering by target name. We should get
    // back an empty list of comms, since we haven't opened any comms with
    // the target name "i-think-not".
    log::info!("Requesting comm info from the kernel for a non-existent comm");
    frontend.send_shell(CommInfoRequest {
        target_name: "i-think-not".to_string(),
    });
    frontend.recv_iopub_busy();

    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            log::info!("Got comm info: {:?}", request);
            let comms = request.content.comms;
            assert!(comms.is_empty());
        },
        _ => {
            panic!(
                "Unexpected message received (expected comm info): {:?}",
                reply
            );
        },
    }

    frontend.recv_iopub_idle();
    frontend.assert_no_incoming();

    log::info!("Sending comm message to the test comm and waiting for a reply");
    let comm_req_id = frontend.send_shell(CommWireMsg {
        comm_id: comm_id.to_string(),
        data: serde_json::Value::Null,
    });

    frontend.recv_iopub_busy();

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
    log::info!("Sending comm close request to the kernel");
    frontend.send_shell(CommClose {
        comm_id: comm_id.to_string(),
    });

    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();

    // Test to see if the comm is still in the list of comms after closing it
    // (it should not be)
    log::info!("Requesting comm info from the kernel (to test closing)");
    frontend.send_shell(CommInfoRequest {
        target_name: "variables".to_string(),
    });
    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            log::info!("Got comm info: {:?}", request);
            // Ensure the comm we just closed not present in the list of comms
            let comms = request.content.comms;
            assert!(!comms.contains_key(comm_id));
        },
        _ => {
            panic!(
                "Unexpected message received (expected comm info): {:?}",
                reply
            );
        },
    }

    // Now test opening a comm from the kernel side
    log::info!("Creating a comm from the kernel side");
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
    //
    // We do this in a loop because we expect a number of other messages, e.g. busy/idle
    loop {
        let msg = frontend.recv_iopub();
        match msg {
            Message::CommOpen(msg) => {
                assert_eq!(msg.content.comm_id, test_comm_id);
                break;
            },
            _ => {
                continue;
            },
        }
    }

    // Query the kernel to see if the comm we just opened is in the list of
    // comms. It's similar to the test done above for opening a comm from the
    // frontend, but this time we're testing the other direction, to ensure that
    // the kernel is correctly tracking the list of comms regardless of where
    // they originated.
    log::info!("Requesting comm info from the kernel (to test opening from the back end)");
    frontend.send_shell(CommInfoRequest {
        target_name: test_comm_name.clone(),
    });
    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            log::info!("Got comm info: {:?}", request);
            // Ensure the comm we just opened is in the list of comms
            let comms = request.content.comms;
            assert!(comms.contains_key(&test_comm_id));

            // Ensure the comm we just opened has the correct target name
            let comm = comms.get(&test_comm_id).unwrap();
            let target = serde_json::from_value::<CommInfoTargetName>(comm.clone()).unwrap();
            assert!(target.target_name == test_comm_name)
        },
        _ => {
            panic!(
                "Unexpected message received (expected comm info): {:?}",
                reply
            );
        },
    }

    // Now send a message from the backend to the frontend using the comm we just
    // created.
    test_comm
        .outgoing_tx
        .send(CommMsg::Data(serde_json::Value::Null))
        .unwrap();

    // Wait for the comm data message to be received by the frontend.
    loop {
        let msg = frontend.recv_iopub();
        match msg {
            Message::CommMsg(msg) => {
                assert_eq!(msg.content.comm_id, test_comm_id);
                break;
            },
            _ => {
                continue;
            },
        }
    }

    // Close the test comm from the backend side
    test_comm.outgoing_tx.send(CommMsg::Close).unwrap();

    // Ensure that the frontend is notified
    loop {
        let msg = frontend.recv_iopub();
        match msg {
            Message::CommClose(msg) => {
                assert_eq!(msg.content.comm_id, test_comm_id);
                break;
            },
            _ => {
                continue;
            },
        }
    }

    frontend.assert_no_incoming();
}
