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
use crate::wire::language_info::LanguageInfo;
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
            let parsed = match Message::to_jupyter_message(msg) {
                Ok(msg) => msg,
                Err(err) => {
                    warn!("Invalid message arrived on shell socket. {}", err);
                    continue;
                }
            };
            Shell::process_message(parsed, socket);
        }
    }

    fn process_message(msg: Message, socket: &zmq::Socket) {
        match msg {
            Message::KernelInfoRequest(req) => Shell::handle_info_request(req, socket),
            _ => warn!("Unexpected message arrived on shell socket: {:?}", msg),
        }
    }

    fn handle_info_request(req: JupyterMessage<KernelInfoRequest>, socket: &zmq::Socket) {
        debug!("Received shell information request: {:?}", req);
        let info = LanguageInfo {
            name: String::from("Echo"),
            version: String::from("1.0"),
            file_extension: String::from(".ech"),
            mimetype: String::from("text/echo"),
            pygments_lexer: String::new(),
            codemirror_mode: String::new(),
            nbconvert_exporter: String::new(),
        };
        let reply = KernelInfoReply {
            status: String::from("ok"),
            banner: format!("Amalthea {}", env!("CARGO_PKG_VERSION")),
            debugger: false,
            protocol_version: String::from("5.0"),
            help_links: Vec::new(),
            language_info: info,
        };

        let msg = req.create_reply(reply);
        if let Err(err) = msg.send(socket, None) {
            warn!("Could not send kernel info reply: {}", err)
        }
    }
}
