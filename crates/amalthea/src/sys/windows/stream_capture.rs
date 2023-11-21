/*
 * stream_capture.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Sender;

use crate::socket::iopub::IOPubMessage;

pub struct StreamCapture {
    _iopub_tx: Sender<IOPubMessage>,
}

impl StreamCapture {
    pub fn new(iopub_tx: Sender<IOPubMessage>) -> Self {
        Self {
            _iopub_tx: iopub_tx,
        }
    }

    pub fn listen(&self) {
        // TODO: Windows
    }
}
