/*
 * executor.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::socket::Socket;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::Status;
use log::warn;
use serde_json::json;
use std::sync::mpsc::{Receiver, Sender};

/// Wrapper for the language execution socket.
pub struct Executor {
    iopub: Socket,
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    execution_count: u32,
}

impl Executor {
    // TODO: iopub should be just a messgae sender, not the whole socket
    pub fn new(iopub: Socket, sender: Sender<Message>, receiver: Receiver<Message>) -> Self {
        Self {
            iopub: iopub,
            sender: sender,
            receiver: receiver,
            execution_count: 0,
        }
    }

    pub fn listen(&mut self) {
        loop {
            let msg = match self.receiver.recv() {
                Ok(s) => s,
                Err(err) => {
                    warn!("Failed to receive message for execution thread: {}", err);
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };
            if let Err(err) = self.process_message(msg) {
                warn!("Could not process execution message: {}", err)
            }
        }
    }

    pub fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        match msg {
            Message::ExecuteRequest(msg) => self.handle_execute_request(msg),
            _ => Err(Error::UnsupportedMessage(String::from("Executor"))),
        }
    }

    pub fn handle_execute_request(
        &mut self,
        msg: JupyterMessage<ExecuteRequest>,
    ) -> Result<(), Error> {
        self.execution_count = self.execution_count + 1;
        let data = json!({"text/plain": msg.content.code });
        msg.send_reply(
            ExecuteResult {
                execution_count: self.execution_count,
                data: data,
                metadata: serde_json::Value::Null,
            },
            &self.iopub,
        )?;

        let reply = Message::ExecuteReply(msg.create_reply(
            ExecuteReply {
                status: Status::Ok,
                execution_count: self.execution_count,
                user_expressions: serde_json::Value::Null,
            },
            &self.iopub.session,
        ));
        if let Err(_) = self.sender.send(reply) {
            Err(Error::SendError(String::from(
                "Could not return execution to shell",
            )))
        } else {
            Ok(())
        }
    }
}
