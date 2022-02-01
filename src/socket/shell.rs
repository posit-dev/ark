/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::socket::signed_socket::SignedSocket;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::is_complete_reply::IsComplete;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::Status;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::language_info::LanguageInfo;
use crate::wire::wire_message::WireMessage;
use hmac::Hmac;
use log::{debug, trace, warn};
use sha2::Sha256;
use std::thread;

pub struct Shell {}

impl Shell {
    pub fn connect(
        &self,
        ctx: &zmq::Context,
        hmac: Option<Hmac<Sha256>>,
        endpoint: String,
    ) -> Result<(), zmq::Error> {
        let socket = ctx.socket(zmq::ROUTER)?;
        socket.bind(&endpoint)?;
        trace!("Binding to shell socket at {}", endpoint);
        thread::spawn(move || {
            Shell::listen(SignedSocket {
                socket: socket,
                hmac: hmac,
            })
        });
        Ok(())
    }

    fn listen(socket: SignedSocket) {
        let mut execution_count: u32 = 0;
        loop {
            debug!("Listening for shell messages");
            let msg = match WireMessage::read_from_socket(&socket) {
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
            Shell::process_message(parsed, &socket, &mut execution_count);
        }
    }

    fn process_message(msg: Message, socket: &SignedSocket, execution_count: &mut u32) {
        match msg {
            Message::KernelInfoRequest(req) => Shell::handle_info_request(req, socket),
            Message::IsCompleteRequest(req) => Shell::handle_is_complete_request(req, socket),
            Message::ExecuteRequest(req) => {
                Shell::handle_execute_request(req, socket, execution_count)
            }
            Message::CompleteRequest(req) => Shell::handle_complete_request(req, socket),
            _ => warn!("Unexpected message arrived on shell socket: {:?}", msg),
        }
    }

    fn handle_execute_request(
        req: JupyterMessage<ExecuteRequest>,
        socket: &SignedSocket,
        execution_count: &mut u32,
    ) {
        *execution_count = *execution_count + 1;
        debug!("Received execution request {:?}", req);
        let reply = ExecuteReply {
            status: Status::Ok,
            execution_count: *execution_count,
            user_expressions: serde_json::Value::Null,
        };
        if let Err(err) = req.send_reply(reply, socket) {
            warn!("Could not send complete reply: {}", err)
        }
    }

    fn handle_is_complete_request(req: JupyterMessage<IsCompleteRequest>, socket: &SignedSocket) {
        debug!("Received request to test code for completeness: {:?}", req);
        // In this echo example, the code is always complete!
        let reply = IsCompleteReply {
            status: IsComplete::Complete,
            indent: String::from(""),
        };
        if let Err(err) = req.send_reply(reply, socket) {
            warn!("Could not send complete reply: {}", err)
        }
    }

    fn handle_info_request(req: JupyterMessage<KernelInfoRequest>, socket: &SignedSocket) {
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
            status: Status::Ok,
            banner: format!("Amalthea {}", env!("CARGO_PKG_VERSION")),
            debugger: false,
            protocol_version: String::from("5.0"),
            help_links: Vec::new(),
            language_info: info,
        };

        if let Err(err) = req.send_reply(reply, socket) {
            warn!("Could not send kernel info reply: {}", err)
        }
    }

    fn handle_complete_request(req: JupyterMessage<CompleteRequest>, socket: &SignedSocket) {
        debug!("Received request to complete code: {:?}", req);
        let reply = CompleteReply {
            matches: Vec::new(),
            status: Status::Ok,
            cursor_start: 0,
            cursor_end: 0,
            metadata: serde_json::Value::Null,
        };
        if let Err(err) = req.send_reply(reply, socket) {
            warn!("Could not send kernel info reply: {}", err)
        }
    }
}
