/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use log::trace;
use std::thread;

pub struct Shell {
    /// The underlying ZeroMQ context
    ctx: zmq::Context,
}

impl Shell {
    pub fn connect(&self, endpoint: String) -> Result<(), zmq::Error> {
        let socket = self.ctx.socket(zmq::ROUTER)?;
        socket.bind(&endpoint)?;
        trace!("Binding to shell socket at {}", endpoint);
        thread::spawn(move || Self::listen(&socket));
        Ok(())
    }

    fn listen(socket: &zmq::Socket) {
        // TODO: we basically want to loop here on receiving a message
    }
}
