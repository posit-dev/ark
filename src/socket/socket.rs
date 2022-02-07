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

pub fn connect<T: Socket>(
    ctx: &zmq::Context,
    endpoint: String,
    session: Session,
) -> Result<SignedSocket, Error> {
    let socket = match ctx.socket(T::kind()) {
        Ok(s) => s,
        Err(err) => return Err(Error::CreateSocketFailed(T::name(), err)),
    };
    trace!("Binding to ZeroMQ '{}' socket at {}", T::name(), endpoint);
    if let Err(err) = socket.bind(&endpoint) {
        return Err(Error::SocketBindError(T::name(), endpoint, err));
    }
    Ok(SignedSocket {
        socket: socket,
        session: session,
    })
}
