/*
 * stream_capture.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

mod platform;

use crossbeam::channel::Sender;

use crate::socket::iopub::IOPubMessage;

pub struct StreamCapture {
    iopub_tx: Sender<IOPubMessage>,
}

impl StreamCapture {
    pub fn new(iopub_tx: Sender<IOPubMessage>) -> Self {
        Self { iopub_tx }
    }
}

pub trait Listen {
    /// Listens to stdout and stderr and sends the output to the IOPub socket.
    /// Does not return.
    fn listen(&self);
}
