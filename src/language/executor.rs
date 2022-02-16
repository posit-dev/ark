/*
 * executor.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::session::Session;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::Status;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use log::{trace, warn};
use serde_json::json;
use std::sync::mpsc::{Receiver, Sender};

/// Wrapper for the language execution socket.
pub struct Executor {
    /// Sends messages to the iopub channel
    iopub_sender: Sender<Message>,

    /// Sends messages (replies) to the Shell channel
    sender: Sender<Message>,

    /// Receives messages from the Shell channel
    receiver: Receiver<Message>,

    /// Session metadata for the execution thread
    session: Session,

    /// A monotonically increasing execution counter
    execution_count: u32,
}

impl Executor {
    pub fn new(
        session: Session,
        iopub: Sender<Message>,
        sender: Sender<Message>,
        receiver: Receiver<Message>,
    ) -> Self {
        Self {
            iopub_sender: iopub,
            sender: sender,
            receiver: receiver,
            execution_count: 0,
            session: session,
        }
    }

    /// Main execution loop for the execution thread
    pub fn listen(&mut self) {
        // Let the front end know that we're ready for business
        trace!("Listening for execution requests");
        if let Err(err) = self
            .iopub_sender
            .send(Message::Status(JupyterMessage::create(
                KernelStatus {
                    execution_state: ExecutionState::Idle,
                },
                None,
                &self.session,
            )))
        {
            warn!("Could not update kernel execution status: {}", err);
        }

        // Process each message received from the shell channel
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

    /// Process a message from the shell thread
    pub fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        match msg {
            Message::ExecuteRequest(msg) => self.handle_execute_request(msg),
            _ => Err(Error::UnsupportedMessage(msg, String::from("Executor"))),
        }
    }

    /// Handle an execution request from the front end
    pub fn handle_execute_request(
        &mut self,
        msg: JupyterMessage<ExecuteRequest>,
    ) -> Result<(), Error> {
        self.execution_count = self.execution_count + 1;

        // For this toy echo language, generate a result that's just the input
        // echoed back.
        let data = json!({"text/plain": msg.content.code });
        if let Err(err) = self
            .iopub_sender
            .send(Message::ExecuteResult(JupyterMessage::create(
                ExecuteResult {
                    execution_count: self.execution_count,
                    data: data,
                    metadata: serde_json::Value::Null,
                },
                Some(msg.header.clone()),
                &self.session,
            )))
        {
            return Err(Error::SendError(format!("{}", err)));
        }

        // Let the shell thread know that we've successfully executed the code.
        let reply = Message::ExecuteReply(msg.create_reply(
            ExecuteReply {
                status: Status::Ok,
                execution_count: self.execution_count,
                user_expressions: serde_json::Value::Null,
            },
            &self.session,
        ));
        if let Err(err) = self.sender.send(reply) {
            Err(Error::SendError(format!(
                "Could not return execution to shell: {}",
                err
            )))
        } else {
            Ok(())
        }
    }
}
