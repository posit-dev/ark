//
// ui.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::base_comm::JsonRpcError;
use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::ui_comm::BusyParams;
use amalthea::comm::ui_comm::CallMethodParams;
use amalthea::comm::ui_comm::UiBackendReply;
use amalthea::comm::ui_comm::UiBackendRequest;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::stdin::StdInRequest;
use ark::r_task::r_task;
use ark::fixtures::r_test;
use ark::ui::UiComm;
use ark::ui::UiCommMessage;
use crossbeam::channel::bounded;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use serde_json::Value;

/**
 * Basic tests for the UI comm.
 */
#[test]
fn test_ui_comm() {
    r_test(|| {
        // Create a sender/receiver pair for the comm channel.
        let comm_socket = CommSocket::new(
            CommInitiator::FrontEnd,
            String::from("test-ui-comm-id"),
            String::from("positron.UI"),
        );

        // Communication channel between the main thread and the Amalthea
        // StdIn socket thread
        let (stdin_request_tx, _stdin_request_rx) = bounded::<StdInRequest>(1);

        // Create a frontend instance
        let ui_comm = UiComm::start(comm_socket.clone(), stdin_request_tx);

        // Get the current console width
        let old_width = r_task(|| unsafe {
            let width = RFunction::from("getOption")
                .param("x", "width")
                .call()
                .unwrap();
            RObject::to::<i32>(width).unwrap()
        });

        // Send a message to the frontend
        let id = String::from("test-id-1");
        let request = UiBackendRequest::CallMethod(CallMethodParams {
            method: String::from("setConsoleWidth"),
            params: vec![Value::from(123)],
        });
        comm_socket
            .incoming_tx
            .send(CommMsg::Rpc(id, serde_json::to_value(request).unwrap()))
            .unwrap();

        // Wait for the reply; this should be a FrontendRpcResult. We don't wait
        // more than a second since this should be quite fast and we don't want to
        // hang the test suite if it doesn't return.
        let response = comm_socket
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let result = serde_json::from_value::<UiBackendReply>(result).unwrap();
                assert_eq!(id, "test-id-1");
                // This RPC should return the old width
                assert_eq!(
                    result,
                    UiBackendReply::CallMethodReply(Value::from(old_width))
                );
            },
            _ => panic!("Unexpected response: {:?}", response),
        }

        // Get the new console width
        let new_width = r_task(|| unsafe {
            let width = RFunction::from("getOption")
                .param("x", "width")
                .call()
                .unwrap();
            RObject::to::<i32>(width).unwrap()
        });

        // Assert that the console width changed
        assert_eq!(new_width, 123);

        // Now try to invoke an RPC that doesn't exist
        let id = String::from("test-id-2");
        let request = UiBackendRequest::CallMethod(CallMethodParams {
            method: String::from("thisRpcDoesNotExist"),
            params: vec![],
        });
        comm_socket
            .incoming_tx
            .send(CommMsg::Rpc(id, serde_json::to_value(request).unwrap()))
            .unwrap();

        // Wait for the reply
        let response = comm_socket
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let _reply = serde_json::from_value::<JsonRpcError>(result).unwrap();
                // Ensure that the error code is -32601 (method not found)
                assert_eq!(id, "test-id-2");

                // TODO: This should normally throw a `MethodNotFound` but
                // that's currently a bit hard because of the nested method
                // call. One way to solve this would be for RPC handler
                // functions to return a typed JSON-RPC error instead of a
                // `anyhow::Result`. Then we could return a `MethodNotFound` from
                // `callMethod()`.
                //
                // assert_eq!(reply.error.code, JsonRpcErrorCode::MethodNotFound);
            },
            _ => panic!("Unexpected response: {:?}", response),
        }

        // Mark not busy (this prevents the frontend comm from being closed due to
        // the Sender being dropped)
        ui_comm
            .send(UiCommMessage::Event(UiFrontendEvent::Busy(BusyParams {
                busy: false,
            })))
            .unwrap();
    });
}
