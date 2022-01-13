/*
 * heartbeat.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

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
        thread::spawn(move || Self::listen(&socket));
        Ok(())
    }

    fn listen(socket: &zmq::Socket) {
        loop {
            println!("listening for heartbeats");
            let mut msg = zmq::Message::new();
            if let Err(err) = socket.recv(&mut msg, 0) {
                // TODO: log error receiving heartbeat
                // TODO: maybe sleep a little to avoid error spam, we
                // wouldn't want this loop to get tight
                println!("error receiving heartbeat: {}", err);
                continue;
            }

            // echo the message right back!
            if let Err(err) = socket.send(msg, 0) {
                println!("error replying to heartbeat: {}", err);
            }
        }
    }
}
