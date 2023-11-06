//
// kernel.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;
use std::result::Result::Err;

use amalthea::events::BusyEvent;
use amalthea::events::PositronEvent;
use amalthea::events::WorkingDirectoryEvent;
use anyhow::Result;
use crossbeam::channel::Sender;
use log::*;

use crate::interface::RMain;
use crate::r_task;
use crate::request::KernelRequest;

/// Represents the Rust state of the R kernel
pub struct Kernel {
    event_tx: Option<Sender<PositronEvent>>,
    working_directory: PathBuf,
}

impl Kernel {
    /// Create a new R kernel instance
    pub fn new() -> Self {
        Self {
            event_tx: None,
            working_directory: PathBuf::new(),
        }
    }

    /// Service an execution request from the front end
    pub fn fulfill_request(&mut self, req: &KernelRequest) {
        match req {
            KernelRequest::EstablishEventChannel(sender) => {
                self.establish_event_handler(sender.clone())
            },
        }
    }

    /// Establishes the event handler for the kernel to send events to the
    /// Positron front end. This event handler is used to send global events
    /// that are not scoped to any particular view. The `Sender` here is a
    /// channel that is connected to a `positron.frontEnd` comm.
    pub fn establish_event_handler(&mut self, event_tx: Sender<PositronEvent>) {
        self.event_tx = Some(event_tx);

        // Clear the current working directory to generate an event for the new
        // client (i.e. after a reconnect)
        self.working_directory = PathBuf::new();
        if let Err(err) = self.poll_working_directory() {
            warn!(
                "Error establishing working directory for front end: {}",
                err
            );
        }

        // Get the current busy status
        let busy = r_task(|| {
            if RMain::initialized() {
                RMain::get().is_busy
            } else {
                false
            }
        });
        self.send_event(PositronEvent::Busy(BusyEvent { busy }));
    }

    /// Polls for changes to the working directory, and sends an event to the
    /// front end if the working directory has changed.
    pub fn poll_working_directory(&mut self) -> Result<()> {
        // Get the current working directory
        let mut current_dir = std::env::current_dir()?;

        // If it isn't the same as the last working directory, send an event
        if current_dir != self.working_directory {
            self.working_directory = current_dir.clone();

            // Attempt to alias the directory, if it's within the home directory
            if let Some(home_dir) = home::home_dir() {
                if let Ok(stripped_dir) = current_dir.strip_prefix(home_dir) {
                    let mut new_path = PathBuf::from("~");
                    new_path.push(stripped_dir);
                    current_dir = new_path;
                }
            }

            // Deliver event to client
            self.send_event(PositronEvent::WorkingDirectory(WorkingDirectoryEvent {
                directory: current_dir.to_string_lossy().to_string(),
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
