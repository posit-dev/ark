//
// frontend.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::base_comm::JsonRpcError;
use amalthea::comm::base_comm::JsonRpcErrorCode;
use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::frontend_comm::BusyParams;
use amalthea::comm::frontend_comm::CallMethodParams;
use amalthea::comm::frontend_comm::FrontendBackendRpcReply;
use amalthea::comm::frontend_comm::FrontendBackendRpcRequest;
use amalthea::comm::frontend_comm::FrontendEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::stdin::StdInRequest;
use ark::frontend::frontend::PositronFrontend;
use ark::frontend::frontend::PositronFrontendMessage;
use ark::r_task;
use ark::test::r_test;
use crossbeam::channel::bounded;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use serde_json::Value;

/**
 * Basic test for the frontend comm.
 */
#[test]
fn test_frontend_comm() {
    r_test(|| {
        // Create a sender/receiver pair for the comm channel.
        let comm = CommSocket::new(
            CommInitiator::FrontEnd,
            String::from("test-frontend-comm-id"),
            String::from("positron.frontend"),
        );

        // Communication channel between the main thread and the Amalthea
        // StdIn socket thread
        let (stdin_request_tx, _stdin_request_rx) = bounded::<StdInRequest>(1);

        // Create a frontend instance
        let frontend = PositronFrontend::start(comm.clone(), stdin_request_tx);

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
        let request = FrontendBackendRpcRequest::CallMethod(CallMethodParams {
            method: String::from("setConsoleWidth"),
            params: vec![Value::from(123)],
        });
        comm.incoming_tx
            .send(CommMsg::Rpc(id, serde_json::to_value(request).unwrap()))
            .unwrap();

        // Wait for the reply; this should be a FrontendRpcResult. We don't wait
        // more than a second since this should be quite fast and we don't want to
        // hang the test suite if it doesn't return.
        let response = comm
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let result = serde_json::from_value::<FrontendBackendRpcReply>(result).unwrap();
                assert_eq!(id, "test-id-1");
                // This RPC should return the old width
                assert_eq!(
                    result,
                    FrontendBackendRpcReply::CallMethodReply(Value::from(old_width))
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
        let request = FrontendBackendRpcRequest::CallMethod(CallMethodParams {
            method: String::from("thisRpcDoesNotExist"),
            params: vec![],
        });
        comm.incoming_tx
            .send(CommMsg::Rpc(id, serde_json::to_value(request).unwrap()))
            .unwrap();

        // Wait for the reply
        let response = comm
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let reply = serde_json::from_value::<JsonRpcError>(result).unwrap();
                // Ensure that the error code is -32601 (method not found)
                assert_eq!(id, "test-id-2");
                assert_eq!(reply.error.code, JsonRpcErrorCode::MethodNotFound);
            },
            _ => panic!("Unexpected response: {:?}", response),
        }

        // Mark not busy (this prevents the frontend comm from being closed due to
        // the Sender being dropped)
        frontend
            .send(PositronFrontendMessage::Event(PositronEvent::Busy(
                BusyParams { busy: false },
            )))
            .unwrap();
    });
}
