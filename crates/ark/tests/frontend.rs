//
// frontend.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::comm::frontend_comm::FrontendMessage;
use amalthea::comm::frontend_comm::FrontendRpcError;
use amalthea::comm::frontend_comm::FrontendRpcRequest;
use amalthea::comm::frontend_comm::FrontendRpcResult;
use amalthea::events::BusyEvent;
use amalthea::events::PositronEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use ark::frontend::frontend::PositronFrontend;
use ark::r_task;
use ark::test::r_test;
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

        // Create a frontend instance
        let frontend = PositronFrontend::start(comm.clone());

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
        let request = FrontendMessage::RpcRequest(FrontendRpcRequest {
            method: String::from("setConsoleWidth"),
            params: vec![Value::from(123)],
        });
        comm.incoming_tx
            .send(CommChannelMsg::Rpc(
                id,
                serde_json::to_value(request).unwrap(),
            ))
            .unwrap();

        // Wait for the reply; this should be a FrontendRpcResult. We don't wait
        // more than a second since this should be quite fast and we don't want to
        // hang the test suite if it doesn't return.
        let response = comm
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommChannelMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let result = serde_json::from_value::<FrontendRpcResult>(result).unwrap();
                assert_eq!(id, "test-id-1");
                // This RPC should return the old width
                assert_eq!(result.result, Value::from(old_width));
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
        let request = FrontendMessage::RpcRequest(FrontendRpcRequest {
            method: String::from("thisRpcDoesNotExist"),
            params: vec![],
        });
        comm.incoming_tx
            .send(CommChannelMsg::Rpc(
                id,
                serde_json::to_value(request).unwrap(),
            ))
            .unwrap();

        // Wait for the reply
        let response = comm
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommChannelMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let error = serde_json::from_value::<FrontendRpcError>(result).unwrap();
                // Ensure that the error code is -32601 (method not found)
                assert_eq!(id, "test-id-2");
                assert_eq!(error.error.code, -32601);
            },
            _ => panic!("Unexpected response: {:?}", response),
        }

        // Mark not busy (this prevents the frontend comm from being closed due to
        // the Sender being dropped)
        frontend
            .send(PositronEvent::Busy(BusyEvent { busy: false }))
            .unwrap();
    });
}
