/*
 * heartbeat.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use log::{debug, trace, warn};
use std::thread;

pub struct Heartbeat {
    /// The underlying ZeroMQ context
    ctx: zmq::Context,
}

impl Heartbeat {
    pub fn create(ctx: zmq::Context) -> Result<Heartbeat, zmq::Error> {
        Ok(Self { ctx: ctx })
    }

    pub fn connect(&self, endpoint: String) -> Result<(), zmq::Error> {
        let socket = self.ctx.socket(zmq::REQ)?;
        socket.bind(&endpoint)?;
        trace!("Binding to heartbeat socket at {}", endpoint);
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
