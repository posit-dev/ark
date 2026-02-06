//
// comm.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use std::time::Duration;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::socket;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::header::JupyterHeader;
use crossbeam::channel::Receiver;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Default timeout for receiving comm messages in tests.
pub const RECV_TIMEOUT: Duration = Duration::from_secs(10);

/// Extension trait for receiving `CommMsg` from `IOPubMessage::CommOutgoing`.
pub trait IOPubReceiverExt {
    /// Receive a comm message with the default timeout (`RECV_TIMEOUT`).
    /// Panics if the timeout expires.
    fn recv_comm_msg(&self) -> CommMsg;

    /// Receive a comm message with a timeout.
    /// Returns `None` if the timeout expires.
    fn recv_comm_msg_timeout(&self, timeout: Duration) -> Option<CommMsg>;
}

/// Create a dummy JupyterHeader for use in tests.
///
/// This allows tests to send `CommMsg::Rpc` with a proper header, matching
/// production behavior where RPCs always have a parent header from the
/// original request.
pub fn dummy_jupyter_header() -> JupyterHeader {
    JupyterHeader::create(
        String::from("comm_msg"),
        String::from("test-session"),
        String::from("test-user"),
    )
}

impl IOPubReceiverExt for Receiver<IOPubMessage> {
    fn recv_comm_msg(&self) -> CommMsg {
        self.recv_comm_msg_timeout(RECV_TIMEOUT)
            .expect("Timed out waiting for CommOutgoing message")
    }

    fn recv_comm_msg_timeout(&self, timeout: Duration) -> Option<CommMsg> {
        match self.recv_timeout(timeout) {
            Ok(IOPubMessage::CommOutgoing(_comm_id, comm_msg)) => Some(comm_msg),
            Ok(other) => panic!("Expected CommOutgoing message, got {:?}", other),
            Err(_) => None,
        }
    }
}

pub fn socket_rpc_request<'de, RequestType, ReplyType>(
    socket: &socket::comm::CommSocket,
    iopub_rx: &Receiver<IOPubMessage>,
    req: RequestType,
) -> ReplyType
where
    RequestType: Serialize,
    ReplyType: DeserializeOwned,
{
    let id = uuid::Uuid::new_v4().to_string();
    let json = serde_json::to_value(req).unwrap();

    let msg = CommMsg::Rpc {
        id,
        parent_header: dummy_jupyter_header(),
        data: json,
    };
    socket.incoming_tx.send(msg).unwrap();

    // Receive the response from IOPub
    let iopub_msg = iopub_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();

    match iopub_msg {
        IOPubMessage::CommOutgoing(_comm_id, CommMsg::Rpc { data: value, .. }) => {
            serde_json::from_value(value).unwrap()
        },
        IOPubMessage::CommOutgoing(_comm_id, CommMsg::Data(value)) => {
            panic!(
                "Expected RPC response but received Data event: {:?}. \
                 The comm may have sent an event before the RPC reply.",
                value
            )
        },
        IOPubMessage::CommOutgoing(_comm_id, CommMsg::Close) => {
            panic!(
                "Expected RPC response but comm was closed. \
                 The comm may have shut down before responding."
            )
        },
        _ => panic!(
            "Expected CommOutgoing with RPC response, got: {:?}. \
             This may indicate the comm routed through a different channel.",
            iopub_msg
        ),
    }
}
