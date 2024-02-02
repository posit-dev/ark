/*
 * heartbeat.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use log::debug;
use log::trace;
use log::warn;

use crate::socket::socket::Socket;

/// Structure used for heartbeat messages
pub struct Heartbeat {
    socket: Socket,
    kernel_heartbeat_rx: Receiver<()>,
    kernel_initialized: bool,
}

impl Heartbeat {
    /// Create a new heartbeat handler from the given heartbeat socket
    pub fn new(socket: Socket, kernel_heartbeat_rx: Receiver<()>) -> Self {
        Self {
            socket,
            kernel_heartbeat_rx,
            kernel_initialized: false,
        }
    }

    /// Listen for heartbeats; does not return
    pub fn listen(&mut self) {
        loop {
            debug!("Listening for heartbeats");
            let mut msg = zmq::Message::new();
            if let Err(err) = self.socket.recv(&mut msg) {
                warn!("Error receiving heartbeat: {}", err);

                // Wait 1s before trying to receive another heartbeat. This
                // keeps us from flooding the logs when recv() isn't working.
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            } else {
                trace!("Heartbeat message: {:?}", msg);
            }

            if !self.kernel_initialized {
                // We've received an initial heartbeat message from the frontend.
                // The frontend uses our first response to set the runtime state
                // to "ready", so we delay our response until the kernel is
                // fully initialized (i.e. until R has fully started up and
                // called our `read_console()` hook at least once).
                match self.kernel_heartbeat_rx.recv() {
                    Ok(_) => log::info!("Received kernel initialization notification. Responding to initial heartbeat."),
                    Err(err) => panic!("Failed to receive kernel initialization notification: {err:?}.")
                }
                self.kernel_initialized = true;
            }

            // Echo the message right back!
            if let Err(err) = self.socket.send(msg) {
                warn!("Error replying to heartbeat: {}", err);
            } else {
                trace!("Heartbeat message replied");
            }
        }
    }
}
