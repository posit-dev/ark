//
// iopub.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use crossbeam::channel::SendError;
use crossbeam::channel::Sender;

use crate::socket::iopub::IOPubMessage;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;

pub trait IOPubSenderExt {
    /// Sets the kernel state by sending a message on the IOPub channel.
    fn send_state<T: ProtocolMessage>(
        &self,
        parent: JupyterMessage<T>,
        state: ExecutionState,
    ) -> Result<(), SendError<IOPubMessage>>;
}

impl IOPubSenderExt for Sender<IOPubMessage> {
    fn send_state<T: ProtocolMessage>(
        &self,
        parent: JupyterMessage<T>,
        state: ExecutionState,
    ) -> Result<(), SendError<IOPubMessage>> {
        let reply = KernelStatus {
            execution_state: state,
        };
        self.send(IOPubMessage::Status(parent.header, reply))
    }
}
