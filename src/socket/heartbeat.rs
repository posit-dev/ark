/*
 * heartbeat.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use log::{debug, trace, warn};
use std::thread;

pub struct Heartbeat {}

impl Heartbeat {
    pub fn connect(&self, ctx: &zmq::Context, endpoint: String) -> Result<(), Error> {
        let socket = match ctx.socket(zmq::REP) {
            Ok(s) => s,
            Err(err) => return Err(Error::CreateSocketFailed(String::from("heartbeat"), err)),
        };
        trace!("Binding to heartbeat socket at {}", endpoint);
        if let Err(err) = socket.bind(&endpoint) {
            return Err(Error::SocketBindError(
                String::from("heartbeat"),
                endpoint,
                err,
            ));
        }
        thread::spawn(move || Self::listen(&socket));
        Ok(())
    }

    fn listen(socket: &zmq::Socket) {
        loop {
            debug!("Listening for heartbeats");
            let mut msg = zmq::Message::new();
            if let Err(err) = socket.recv(&mut msg, 0) {
                warn!("Error receiving heartbeat: {}", err);

                // Wait 1s before trying to receive another heartbeat. This
                // keeps us from flooding the logs when recv() isn't working.
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            } else {
                debug!("Heartbeat message: {:?}", msg);
            }

            // Echo the message right back!
            if let Err(err) = socket.send(msg, 0) {
                warn!("Error replying to heartbeat: {}", err);
            } else {
                debug!("Heartbeat message replied");
            }
        }
    }
}
