/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::wire_message::WireMessage;
use log::{debug, trace, warn};
use std::thread;

pub struct Shell {}

impl Shell {
    pub fn connect(&self, ctx: &zmq::Context, endpoint: String) -> Result<(), zmq::Error> {
        let socket = ctx.socket(zmq::ROUTER)?;
        socket.bind(&endpoint)?;
        trace!("Binding to shell socket at {}", endpoint);
        thread::spawn(move || Self::listen(&socket));
        Ok(())
    }

    fn listen(socket: &zmq::Socket) {
        loop {
            debug!("Listening for shell messages");
            let msg = match WireMessage::read_from_socket(socket, None) {
                Ok(msg) => msg,
                Err(err) => {
                    warn!("Error reading shell message. {}", err);
                    continue;
                }
            };
            let parsed = match msg.to_jupyter_message() {
                Ok(msg) => msg,
                Err(err) => {
                    warn!("Invalid message arrived on shell socket. {}", err);
                    continue;
                }
            };
            Shell::process_message(parsed);
        }
    }

    fn process_message(msg: Message) {
        match msg {
            Message::KernelInfoRequest(req) => Shell::handle_info_request(req),
            _ => warn!("Unexpected message arrived on shell socket: {:?}", msg),
        }
    }

    fn handle_info_request(req: JupyterMessage<KernelInfoRequest>) {
        debug!("Received shell information request: {:?}", req);
    }
}
