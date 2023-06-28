//
// kernel.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::result::Result::Err;

use amalthea::events::PositronEvent;
use amalthea::socket::iopub::IOPubMessage;
use crossbeam::channel::Sender;
use log::*;

use crate::request::KernelRequest;

/// Represents the Rust state of the R kernel
pub struct Kernel {
    iopub_tx: Sender<IOPubMessage>,
    event_tx: Option<Sender<PositronEvent>>,
}

impl Kernel {
    /// Create a new R kernel instance
    pub fn new(iopub_tx: Sender<IOPubMessage>) -> Self {
        Self {
            iopub_tx,
            event_tx: None,
        }
    }

    /// Service an execution request from the front end
    pub fn fulfill_request(&mut self, req: &KernelRequest) {
        match req {
            KernelRequest::EstablishEventChannel(sender) => {
                self.establish_event_handler(sender.clone())
            },
            KernelRequest::DeliverEvent(event) => self.handle_event(event),
        }
    }

    /// Handle an event from the back end to the front end
    pub fn handle_event(&mut self, event: &PositronEvent) {
        if let Err(err) = self.iopub_tx.send(IOPubMessage::Event(event.clone())) {
            warn!("Error attempting to deliver client event: {}", err);
        }
    }

    /// Establishes the event handler for the kernel to send events to the
    /// Positron front end. This event handler is used to send global events
    /// that are not scoped to any particular view. The `Sender` here is a
    /// channel that is connected to a `positron.frontEnd` comm.
    pub fn establish_event_handler(&mut self, event_tx: Sender<PositronEvent>) {
        self.event_tx = Some(event_tx);
    }

    /// Sends an event to the front end (Positron-specific)
    pub fn send_event(&self, event: PositronEvent) {
        info!("Sending Positron event: {:?}", event);
        if let Some(event_tx) = &self.event_tx {
            if let Err(err) = event_tx.send(event) {
                warn!("Error sending event to front end: {}", err);
            }
        } else {
            warn!(
                "Discarding event {:?}; no Positron front end connected",
                event
            );
        }
    }
}
