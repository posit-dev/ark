/*
 * client.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::fixtures::dummy_frontend::DummyConnection;
use amalthea::fixtures::dummy_frontend::DummyFrontend;
use amalthea::kernel;
use amalthea::kernel::StreamBehavior;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::socket::stdin::StdInRequest;
use amalthea::wire::comm_close::CommClose;
use amalthea::wire::comm_info_reply::CommInfoTargetName;
use amalthea::wire::comm_info_request::CommInfoRequest;
use amalthea::wire::comm_msg::CommWireMsg;
use amalthea::wire::comm_open::CommOpen;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_result::ExecuteResult;
use amalthea::wire::input_reply::InputReply;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::jupyter_message::MessageType;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::status::ExecutionState;
use amalthea::wire::status::KernelStatus;
use amalthea::wire::stream::StreamOutput;
use amalthea::wire::wire_message::WireMessage;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use log::info;
use serde_json;

mod control;
mod shell;

#[test]
fn test_kernel() {
    // Let's skip this test on Windows for now to see if the Host Unreachable
    // error only happens here
    #[cfg(target_os = "windows")]
    return;

    let connection = DummyConnection::new();
    let (connection_file, registration_file) = connection.get_connection_files();

    let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(10);

    let (stdin_request_tx, stdin_request_rx) = bounded::<StdInRequest>(1);
    let (stdin_reply_tx, stdin_reply_rx) = unbounded();

    let shell = Arc::new(Mutex::new(shell::Shell::new(
        iopub_tx.clone(),
        stdin_request_tx,
        stdin_reply_rx,
    )));
    let control = Arc::new(Mutex::new(control::Control {}));

    // Initialize logging
    env_logger::init();
    info!("Starting test kernel");

    // Perform kernel connection on its own thread to
    // avoid deadlocking as it waits for the `HandshakeReply`
    stdext::spawn!("dummy_kernel", {
        let comm_manager_tx = comm_manager_tx.clone();

        move || {
            if let Err(err) = kernel::connect(
                "amalthea",
                connection_file,
                Some(registration_file),
                shell,
                control,
                None,
                None,
                StreamBehavior::None,
                iopub_tx,
                iopub_rx,
                comm_manager_tx,
                comm_manager_rx,
                stdin_request_rx,
                stdin_reply_tx,
            ) {
                panic!("Error connecting kernel: {err:?}");
            };
        }
    });

    // Complete client initialization
    info!("Creating frontend");
    let frontend = DummyFrontend::from_connection(connection);

    // Ask the kernel for the kernel info. This should return an object with the
    // language "Test" defined in our shell handler.
    info!("Requesting kernel information");
    frontend.send_shell(KernelInfoRequest {});

    info!("Waiting for kernel info reply");
    let reply = frontend.recv_shell();
    match reply {
        Message::KernelInfoReply(reply) => {
            info!("Kernel info received: {:?}", reply);
            assert_eq!(reply.content.language_info.name, "Test");
        },
        _ => {
            panic!("Unexpected message received: {:?}", reply);
        },
    }

    // Ask the kernel to execute some code
    info!("Requesting execution of code '42'");
    frontend.send_shell(ExecuteRequest {
        code: "42".to_string(),
        silent: false,
        store_history: true,
        user_expressions: serde_json::Value::Null,
        allow_stdin: false,
        stop_on_error: false,
    });

    // The kernel should send an execute reply message indicating that the execute succeeded
    info!("Waiting for execute reply");
    let reply = frontend.recv_shell();
    match reply {
        Message::ExecuteReply(reply) => {
            info!("Received execute reply: {:?}", reply);
            assert_eq!(reply.content.status, Status::Ok);
            assert_eq!(reply.content.execution_count, 1);
        },
        _ => {
            panic!("Unexpected execute reply received: {:?}", reply);
        },
    }

    // The IOPub channel should receive six messages, in this order:
    // 1. A message indicating that the kernel has entered the busy state
    //    (for the kernel_info_request)
    // 2. A message indicating that the kernel has entered the idle state
    //    (for the kernel_info_request)
    // 3. A message indicating that the kernel has entered the busy state
    //    (for the execute_request)
    // 4. A message re-broadcasting the input
    // 5. A message with the result of the execution
    // 6. A message indicating that the kernel has exited the busy state
    //    (for the execute_request)

    info!("Waiting for IOPub execution information messsage 1 of 6: Status");
    let iopub_1 = frontend.recv_iopub();
    match iopub_1 {
        Message::Status(status) => {
            info!("Got kernel status: {:?}", status);
            // TODO: validate parent header
            assert_eq!(status.content.execution_state, ExecutionState::Busy);
        },
        _ => {
            panic!(
                "Unexpected message received (expected status): {:?}",
                iopub_1
            );
        },
    }

    info!("Waiting for IOPub execution information messsage 2 of 6: Status");
    let iopub_2 = frontend.recv_iopub();
    match iopub_2 {
        Message::Status(status) => {
            info!("Got kernel status: {:?}", status);
            // TODO: validate parent header
            assert_eq!(status.content.execution_state, ExecutionState::Idle);
        },
        _ => {
            panic!(
                "Unexpected message received (expected status): {:?}",
                iopub_2
            );
        },
    }

    info!("Waiting for IOPub execution information messsage 3 of 6: Status");
    let iopub_3 = frontend.recv_iopub();
    match iopub_3 {
        Message::Status(status) => {
            info!("Got kernel status: {:?}", status);
            assert_eq!(status.content.execution_state, ExecutionState::Busy);
        },
        _ => {
            panic!(
                "Unexpected message received (expected status): {:?}",
                iopub_3
            );
        },
    }

    info!("Waiting for IOPub execution information messsage 4 of 6: Input Broadcast");
    let iopub_4 = frontend.recv_iopub();
    match iopub_4 {
        Message::ExecuteInput(input) => {
            info!("Got input rebroadcast: {:?}", input);
            assert_eq!(input.content.code, "42");
        },
        _ => {
            panic!(
                "Unexpected message received (expected input rebroadcast): {:?}",
                iopub_4
            );
        },
    }

    info!("Waiting for IOPub execution information messsage 5 of 6: Execution Result");
    let iopub_5 = frontend.recv_iopub();
    match iopub_5 {
        Message::ExecuteResult(result) => {
            info!("Got execution result: {:?}", result);
        },
        _ => {
            panic!(
                "Unexpected message received (expected execution result): {:?}",
                iopub_5
            );
        },
    }

    info!("Waiting for IOPub execution information messsage 6 of 6: Status");
    let iopub_6 = frontend.recv_iopub();
    match iopub_6 {
        Message::Status(status) => {
            info!("Got kernel status: {:?}", status);
            assert_eq!(status.content.execution_state, ExecutionState::Idle);
        },
        _ => {
            panic!(
                "Unexpected message received (expected status): {:?}",
                iopub_6
            );
        },
    }

    info!("Sending request to generate an input prompt");
    frontend.send_shell(ExecuteRequest {
        code: "prompt".to_string(),
        silent: false,
        store_history: true,
        user_expressions: serde_json::Value::Null,
        allow_stdin: true,
        stop_on_error: false,
    });

    info!("Waiting for kernel to send an input request");
    let request = frontend.recv_stdin();
    match request {
        Message::InputRequest(request) => {
            info!("Got input request: {:?}", request);
            assert_eq!(request.content.prompt, "Amalthea Echo> ");
        },
        _ => {
            panic!(
                "Unexpected message received (expected input request): {:?}",
                request
            );
        },
    }

    info!("Sending input to the kernel");
    frontend.send_stdin(InputReply {
        value: "42".to_string(),
    });

    // Consume the IOPub messages that the kernel sends back during the
    // processing of the above `prompt` execution request
    assert_eq!(
        // Status: Busy
        WireMessage::try_from(&frontend.recv_iopub())
            .unwrap()
            .message_type(),
        KernelStatus::message_type()
    );
    assert_eq!(
        // ExecuteInput (re-broadcast of 'Prompt')
        WireMessage::try_from(&frontend.recv_iopub())
            .unwrap()
            .message_type(),
        ExecuteInput::message_type()
    );
    assert_eq!(
        // StreamOutput (echoed input)
        WireMessage::try_from(&frontend.recv_iopub())
            .unwrap()
            .message_type(),
        StreamOutput::message_type()
    );
    assert_eq!(
        // ExecuteResult
        WireMessage::try_from(&frontend.recv_iopub())
            .unwrap()
            .message_type(),
        ExecuteResult::message_type()
    );
    assert_eq!(
        // Status: Idle
        WireMessage::try_from(&frontend.recv_iopub())
            .unwrap()
            .message_type(),
        KernelStatus::message_type()
    );

    // The kernel should send an execute reply message indicating that the execute
    // of the 'prompt' command succeeded
    info!("Waiting for execute reply");
    let reply = frontend.recv_shell();
    match reply {
        Message::ExecuteReply(reply) => {
            info!("Received execute reply: {:?}", reply);
            assert_eq!(reply.content.status, Status::Ok);
            assert_eq!(reply.content.execution_count, 2);
        },
        _ => {
            panic!("Unexpected execute reply received: {:?}", reply);
        },
    }

    // Test the heartbeat
    info!("Sending heartbeat to the kernel");
    let msg = zmq::Message::from("Heartbeat");
    frontend.send_heartbeat(msg);

    info!("Waiting for heartbeat reply");
    let reply = frontend.recv_heartbeat();
    assert_eq!(reply, zmq::Message::from("Heartbeat"));

    // Test the comms
    info!("Sending comm open request to the kernel");
    let comm_id = "A3A6D0EA-1443-4F70-B059-F423E445B8D6";
    frontend.send_shell(CommOpen {
        comm_id: comm_id.to_string(),
        target_name: "variables".to_string(),
        data: serde_json::Value::Null,
    });

    // Absorb the IOPub messages that the kernel sends back during the
    // processing of the above `CommOpen` request
    frontend.recv_iopub(); // Busy
    frontend.recv_iopub(); // Idle

    info!("Requesting comm info from the kernel (to test opening from the frontend)");
    frontend.send_shell(CommInfoRequest {
        target_name: "".to_string(),
    });
    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            info!("Got comm info: {:?}", request);
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

    // Test requesting comm info and filtering by target name. We should get
    // back an empty list of comms, since we haven't opened any comms with
    // the target name "i-think-not".
    info!("Requesting comm info from the kernel for a non-existent comm");
    frontend.send_shell(CommInfoRequest {
        target_name: "i-think-not".to_string(),
    });
    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            info!("Got comm info: {:?}", request);
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

    info!("Sending comm message to the test comm and waiting for a reply");
    let comm_req_id = frontend.send_shell(CommWireMsg {
        comm_id: comm_id.to_string(),
        data: serde_json::Value::Null,
    });
    loop {
        let msg = frontend.recv_iopub();
        match msg {
            Message::CommMsg(msg) => {
                // This is the message we were looking for; break out of the
                // loop
                info!("Got comm message: {:?}", msg);

                // Ensure that the comm ID in the message matches the comm ID we
                // sent
                assert_eq!(msg.content.comm_id, comm_id);

                // Ensure that the parent message ID in the message exists and
                // matches the message ID of the comm message we sent; this is
                // how RPC responses are aligned with requests
                assert_eq!(msg.parent_header.unwrap().msg_id, comm_req_id);
                break;
            },
            _ => {
                // It isn't the message; keep looking for it (we expect a
                // number of other messages, e.g. busy/idle notifications as
                // the kernel processes the comm message)
                info!("Ignoring message: {:?}", msg);
                continue;
            },
        }
    }

    // Test closing the comm we just opened
    info!("Sending comm close request to the kernel");
    frontend.send_shell(CommClose {
        comm_id: comm_id.to_string(),
    });

    // Absorb the IOPub messages that the kernel sends back during the
    // processing of the above `CommClose` request
    info!("Receiving comm close IOPub messages from the kernel");
    frontend.recv_iopub(); // Busy
    frontend.recv_iopub(); // Idle

    // Test to see if the comm is still in the list of comms after closing it
    // (it should not be)
    info!("Requesting comm info from the kernel (to test closing)");
    frontend.send_shell(CommInfoRequest {
        target_name: "variables".to_string(),
    });
    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            info!("Got comm info: {:?}", request);
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
    info!("Creating a comm from the kernel side");
    let test_comm_id = String::from("test_comm_id_84e7fe");
    let test_comm_name = String::from("test_target");
    let test_comm = CommSocket::new(
        CommInitiator::BackEnd,
        test_comm_id.clone(),
        test_comm_name.clone(),
    );
    comm_manager_tx
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
    info!("Requesting comm info from the kernel (to test opening from the back end)");
    frontend.send_shell(CommInfoRequest {
        target_name: test_comm_name.clone(),
    });
    let reply = frontend.recv_shell();
    match reply {
        Message::CommInfoReply(request) => {
            info!("Got comm info: {:?}", request);
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
}
