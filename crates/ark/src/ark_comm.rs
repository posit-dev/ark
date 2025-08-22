//
// test_handler.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::cell::Cell;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::socket::comm::CommSocket;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ArkBackendEvent {
    #[serde(rename = "test_notification")]
    TestNotification(TestParams),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ArkFrontendEvent {
    #[serde(rename = "test_notification")]
    TestNotification(TestParams),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ArkBackendRequest {
    #[serde(rename = "test_request")]
    TestRequest(TestParams),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum ArkBackendReply {
    #[serde(rename = "test_reply")]
    TestReply(TestParams),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TestParams {
    pub i: i32,
}

pub struct ArkComm {
    comm: CommSocket,
    closed: Cell<bool>,
}

impl ArkComm {
    /// Handle opening an "ark" comm, currently only for testing raw comm functionality
    pub fn handle_comm_open(comm: CommSocket) -> amalthea::Result<bool> {
        log::info!("Opening Ark comm: {}", comm.comm_id);

        let mut comm = Self {
            comm,
            closed: Cell::new(false),
        };
        stdext::spawn!("ark-comm", move || { comm.process_messages() });

        Ok(true)
    }

    pub fn process_messages(&mut self) {
        loop {
            if self.closed.get() {
                break;
            }
            let Ok(msg) = self.comm.incoming_rx.recv() else {
                break;
            };

            log::trace!("Ark Comm: Received message from front end: {msg:?}");

            match msg {
                CommMsg::Data(data) => {
                    let Ok(event) = serde_json::from_value::<ArkBackendEvent>(data.clone()) else {
                        log::warn!("Unknown message {data:?}");
                        continue;
                    };

                    if let Err(err) = self.handle_event(event) {
                        log::warn!("Ark Comm: Error while handling event: {err:?}");
                    }
                },

                CommMsg::Rpc(..) => {
                    self.comm.handle_request(msg, |req| Self::handle_rpc(req));
                },

                CommMsg::Close => {
                    log::trace!("Ark Comm: Received a close message.");
                    break;
                },
            }
        }

        log::info!("Ark Comm: Channel closed");
    }

    fn handle_event(&self, event: ArkBackendEvent) -> anyhow::Result<()> {
        match event {
            ArkBackendEvent::TestNotification(TestParams { i }) => {
                self.send_event(ArkFrontendEvent::TestNotification(TestParams { i: -i }))?;
            },
        };

        Ok(())
    }

    fn handle_rpc(request: ArkBackendRequest) -> anyhow::Result<ArkBackendReply> {
        match request {
            ArkBackendRequest::TestRequest(TestParams { i }) => {
                Ok(ArkBackendReply::TestReply(TestParams { i: -i }))
            },
        }
    }

    fn send_event(&self, message: ArkFrontendEvent) -> anyhow::Result<()> {
        let event = serde_json::to_value(message)?;

        if let Err(_) = self.comm.outgoing_tx.send(CommMsg::Data(event)) {
            self.closed.set(true);
        }

        Ok(())
    }
}
