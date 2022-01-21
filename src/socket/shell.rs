/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

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
        // TODO: we basically want to loop here on receiving a message
        loop {
            debug!("Listening for shell messages");
            let mut msg = zmq::Message::new();
            if let Err(err) = socket.recv(&mut msg, 0) {
                warn!("Error receiving socket message on shell: {}", err);
            } else {
            }
        }
    }
}
