/*
 * socket.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::signed_socket::SignedSocket;
use crate::wire::jupyter_message::Message;
use crate::wire::wire_message::WireMessage;
use hmac::Hmac;
use log::{debug, trace, warn};
use sha2::Sha256;
use std::rc::Rc;
use std::thread;

pub trait Socket {
    fn create(socket: Rc<SignedSocket>) -> Self;
    fn kind() -> zmq::SocketType;
    fn name() -> String;
    fn process_message(&mut self, message: Message) -> Result<(), Error>;
}

pub fn connect<T: Socket>(
    ctx: &zmq::Context,
    endpoint: String,
    hmac: Option<Hmac<Sha256>>,
) -> Result<(), Error> {
    let socket = match ctx.socket(T::kind()) {
        Ok(s) => s,
        Err(err) => return Err(Error::CreateSocketFailed(T::name(), err)),
    };
    trace!("Binding to ZeroMQ '{}' socket at {}", T::name(), endpoint);
    if let Err(err) = socket.bind(&endpoint) {
        return Err(Error::SocketBindError(T::name(), endpoint, err));
    }
    thread::spawn(move || {
        let signed = Rc::new(SignedSocket {
            socket: socket,
            hmac: hmac,
        });
        let mut listener = T::create(signed.clone());
        listen(&mut listener, signed.clone());
    });
    Ok(())
}

fn listen<T: Socket>(listener: &mut T, socket: Rc<SignedSocket>) {
    loop {
        debug!("Listening for messages on {} socket...", T::name());
        let msg = match WireMessage::read_from_socket(socket.as_ref()) {
            Ok(msg) => msg,
            Err(err) => {
                warn!("Error reading {} message. {}", T::name(), err);
                continue;
            }
        };
        let parsed = match Message::to_jupyter_message(msg) {
            Ok(msg) => msg,
            Err(err) => {
                warn!("Invalid message arrived on {} socket. {}", T::name(), err);
                continue;
            }
        };
        if let Err(err) = listener.process_message(parsed) {
            warn!("Could not process message on {} socket: {}", T::name(), err)
        }
    }
}
