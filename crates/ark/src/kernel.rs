//
// kernel.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::result::Result::Err;

use amalthea::events::BusyEvent;
use amalthea::events::PositronEvent;
use amalthea::events::WorkingDirectoryEvent;
use amalthea::socket::iopub::IOPubMessage;
use anyhow::Result;
use crossbeam::channel::Sender;
use log::*;

use crate::interface::RMain;
use crate::r_task;
use crate::request::KernelRequest;

/// Represents the Rust state of the R kernel
pub struct Kernel {
    iopub_tx: Sender<IOPubMessage>,
    event_tx: Option<Sender<PositronEvent>>,
    working_directory: String,
}

impl Kernel {
    /// Create a new R kernel instance
    pub fn new(iopub_tx: Sender<IOPubMessage>) -> Self {
        Self {
            iopub_tx,
            event_tx: None,
            working_directory: String::new(),
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

        // Clear the current working directory to generate an event for the new
        // client
        self.working_directory = String::new();
        if let Err(err) = self.poll_working_directory() {
            warn!(
                "Error establishing working directory for front end: {}",
                err
            );
        }

        // Get the current busy status
        let busy = r_task(|| {
            let main = RMain::get();
            main.is_busy
        });
        self.send_event(PositronEvent::Busy(BusyEvent { busy }));
    }

    /// Polls for changes to the working directory, and sends an event to the
    /// front end if the working directory has changed.
    pub fn poll_working_directory(&mut self) -> Result<()> {
        // Get the current working directory
        let current_dir = std::env::current_dir()?;
        let current_dir = current_dir.to_string_lossy();

        // If it isn't the same as the last working directory, send an event
        if current_dir != self.working_directory {
            self.working_directory = String::from(current_dir);
            self.send_event(PositronEvent::WorkingDirectory(WorkingDirectoryEvent {
                directory: self.working_directory.clone(),
            }));
        };
        Ok(())
    }

    /// Check if the Positron front end is connected
    pub fn positron_connected(&self) -> bool {
        self.event_tx.is_some()
    }

    /// Sends an event to the front end (Positron-specific)
    pub fn send_event(&self, event: PositronEvent) {
        info!("Sending Positron event: {:?}", event);
        if self.positron_connected() {
            let event_tx = self.event_tx.as_ref().unwrap();
            if let Err(err) = event_tx.send(event) {
                warn!("Error sending event to front end: {}", err);
            }
        } else {
            info!(
                "Discarding event {:?}; no Positron front end connected",
                event
            );
        }
    }
}
