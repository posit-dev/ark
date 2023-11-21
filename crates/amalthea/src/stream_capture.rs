/*
 * stream_capture.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Sender;

use crate::socket::iopub::IOPubMessage;
use crate::sys;

/// StreamCapture captures the output of a stream and sends it to the IOPub
/// socket.
pub struct StreamCapture(sys::stream_capture::StreamCapture);

impl StreamCapture {
    pub fn new(iopub_tx: Sender<IOPubMessage>) -> Self {
        StreamCapture(sys::stream_capture::StreamCapture::new(iopub_tx))
    }

    /// Listens to stdout and stderr and sends the output to the IOPub socket.
    /// Does not return.
    pub fn listen(&self) {
        self.0.listen()
    }
}
