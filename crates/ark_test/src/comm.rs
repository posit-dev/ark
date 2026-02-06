//
// comm.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::socket;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub fn socket_rpc_request<'de, RequestType, ReplyType>(
    socket: &socket::comm::CommSocket,
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
        parent_header: None,
        data: json,
    };
    socket.incoming_tx.send(msg).unwrap();
    let msg = socket
        .outgoing_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    match msg {
        CommMsg::Rpc { data: value, .. } => serde_json::from_value(value).unwrap(),
        _ => panic!("Unexpected Comm Message"),
    }
}
