/*
 * socket.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::session::Session;
use crate::socket::signed_socket::SignedSocket;
use log::trace;

pub trait Socket {
    fn kind() -> zmq::SocketType;
    fn name() -> String;
}
