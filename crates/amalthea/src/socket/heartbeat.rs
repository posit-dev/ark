/*
 * heartbeat.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use crate::socket::socket::Socket;

/// Structure used for heartbeat messages
pub struct Heartbeat {
    socket: Socket,
}

impl Heartbeat {
    /// Create a new heartbeat handler from the given heartbeat socket
    pub fn new(socket: Socket) -> Self {
        Self { socket }
    }

    /// Listen for heartbeats; does not return
    pub fn listen(&self) {
        #[cfg(debug_assertions)]
        let quiet = true;
        #[cfg(not(debug_assertions))]
        let quiet = std::env::var("ARK_HEARTBEAT_QUIET").is_ok();

        loop {
            if !quiet {
                log::trace!("Listening for heartbeats");
            }

            let mut msg = zmq::Message::new();
            if let Err(err) = self.socket.recv(&mut msg) {
                log::warn!("Error receiving heartbeat: {}", err);

                // Wait 1s before trying to receive another heartbeat. This
                // keeps us from flooding the logs when recv() isn't working.
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            if !quiet {
                log::trace!("Heartbeat message: {:?}", msg);
            }

            // Echo the message right back!
            if let Err(err) = self.socket.send(msg) {
                log::warn!("Error replying to heartbeat: {}", err);
                continue;
            }
            if !quiet {
                log::trace!("Heartbeat message replied");
            }
        }
    }
}
