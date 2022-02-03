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
use std::rc::Rc;

pub struct IOPub {
    socket: Rc<SignedSocket>,
}

impl Socket for IOPub {
    fn name() -> String {
        String::from("IOPub")
    }

    fn kind() -> zmq::SocketType {
        zmq::ROUTER
    }

    fn create(socket: Rc<SignedSocket>) -> Self {
        Self { socket: socket }
    }

    fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        match msg {
            _ => Err(Error::UnsupportedMessage(Self::name())),
        }
    }
}
