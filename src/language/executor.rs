/*
 * executor.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::signed_socket::SignedSocket;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::Status;
use log::warn;
use serde_json::json;
use std::sync::mpsc::{Receiver, Sender};

struct Executor {
    iopub: SignedSocket,
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    execution_count: u32,
}

impl Executor {
    pub fn new(iopub: SignedSocket, sender: Sender<Message>, receiver: Receiver<Message>) -> Self {
        Self {
            iopub: iopub,
            sender: sender,
            receiver: receiver,
            execution_count: 0,
        }
    }

    pub fn listen(&self) -> Result<(), Error> {
        loop {
            let msg = match self.receiver.recv() {
                Ok(s) => s,
                Err(err) => {
                    warn!("Failed to receive message for execution thread: {}", err);
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };
            self.process_message(msg);
        }
    }

    pub fn process_message(&self, msg: Message) -> Result<(), Error> {
        match msg {
            Message::ExecuteRequest(msg) => self.handle_execute_request(msg),
            _ => Err(Error::UnsupportedMessage(String::from("Executor"))),
        };
        Ok(())
    }

    pub fn handle_execute_request(&self, msg: JupyterMessage<ExecuteRequest>) -> Result<(), Error> {
        let data = json!({"text/plain": msg.content.code });
        msg.send_reply(
            ExecuteResult {
                execution_count: self.execution_count,
                data: data,
                metadata: serde_json::Value::Null,
            },
            &self.iopub,
        );

        // create reply -- note use of create instead of reply since we need to
        // drop zmq identities
        let reply = Message::ExecuteReply(JupyterMessage::create(
            ExecuteReply {
                status: Status::Ok,
                execution_count: self.execution_count,
                user_expressions: serde_json::Value::Null,
            },
            Some(msg.header),
            &self.iopub.session,
        ));
        self.sender.send(reply);
        // TODO error handling
        Ok(())
    }
}
