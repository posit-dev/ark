/*
 * iopub.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::signed_socket::SignedSocket;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::Message;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use std::rc::Rc;

pub struct IOPub {
    socket: Rc<SignedSocket>,
}

impl Socket for IOPub {
    fn name() -> String {
        String::from("IOPub")
    }

    fn kind() -> zmq::SocketType {
        zmq::PUB
    }

    fn create(socket: Rc<SignedSocket>) -> Self {
        Self { socket: socket }
    }

    fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        // The IOPub socket is PUB/SUB, so it publishes messages, and doesn't
        // expect to receive any.
        match msg {
            _ => Err(Error::UnsupportedMessage(Self::name())),
        }
    }
}
