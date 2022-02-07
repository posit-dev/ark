/*
 * heartbeat.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::signed_socket::SignedSocket;
use crate::socket::socket::Socket;
use log::{debug, trace, warn};
use std::rc::Rc;

pub struct Heartbeat {
    socket: Rc<SignedSocket>,
}

impl Socket for Heartbeat {
    fn name() -> String {
        String::from("Heartbeat")
    }

    fn kind() -> zmq::SocketType {
        zmq::REP
    }
}

impl Heartbeat {
    pub fn new(socket: Rc<SignedSocket>) -> Self {
        Self { socket: socket }
    }

    pub fn listen(&mut self) {
        loop {
            debug!("Listening for heartbeats");
            let mut msg = zmq::Message::new();
            if let Err(err) = self.socket.socket.recv(&mut msg, 0) {
                warn!("Error receiving heartbeat: {}", err);

                // Wait 1s before trying to receive another heartbeat. This
                // keeps us from flooding the logs when recv() isn't working.
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            } else {
                debug!("Heartbeat message: {:?}", msg);
            }

            // Echo the message right back!
            if let Err(err) = self.socket.socket.send(msg, 0) {
                warn!("Error replying to heartbeat: {}", err);
            } else {
                debug!("Heartbeat message replied");
            }
        }
    }
}
