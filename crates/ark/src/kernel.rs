//
// kernel.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;
use std::result::Result::Err;
use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::ui_comm::BusyParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::comm::ui_comm::WorkingDirectoryParams;
use amalthea::wire::input_request::UiCommFrontendRequest;
use anyhow::Result;
use crossbeam::channel::Sender;

use crate::interface::RMain;
use crate::r_task;
use crate::request::KernelRequest;
use crate::ui::UiCommMessage;

/// Represents the Rust state of the R kernel
pub struct Kernel {
    ui_comm_tx: Option<Sender<UiCommMessage>>,
    working_directory: PathBuf,
    /// A self reference to send the kernel across threads
    kernel: Option<Arc<Mutex<Kernel>>>,
}

impl Kernel {
    /// Create a new R kernel instance
    pub fn new() -> Arc<Mutex<Kernel>> {
        let kernel = Self {
            ui_comm_tx: None,
            working_directory: PathBuf::new(),
            kernel: None,
        };

        // Initialize self reference
        let kernel = Arc::new(Mutex::new(kernel));
        kernel.lock().unwrap().kernel = Some(kernel.clone());

        kernel
    }

    /// Service an execution request from the frontend
    pub fn fulfill_request(&mut self, req: &KernelRequest) {
        match req {
            KernelRequest::EstablishUiCommChannel(sender) => {
                self.establish_ui_comm_channel(sender.clone())
            },
        }
    }

    /// Establishes the event handler for the kernel to send UI events to the
    /// Positron frontend. This event handler is used to send global events
    /// that are not scoped to any particular view. The `Sender` here is a
    /// channel that is connected to a `positron.frontEnd` comm.
    pub fn establish_ui_comm_channel(&mut self, ui_comm_tx: Sender<UiCommMessage>) {
        self.ui_comm_tx = Some(ui_comm_tx);

        // Clear the current working directory to generate an event for the new
        // client (i.e. after a reconnect)
        self.working_directory = PathBuf::new();
        if let Err(err) = self.poll_working_directory() {
            log::error!("Error establishing working directory for frontend: {err:?}");
        }

        // We shouldn't block with an `r_task()` while holding the kernel lock.
        // So check for status in an async task and send event from there.
        let kernel = self.kernel.as_ref().unwrap().clone();

        r_task::spawn_interrupt(|| async move {
            // Get the current busy status
            let busy = if RMain::initialized() {
                RMain::get().is_busy
            } else {
                false
            };

            kernel
                .lock()
                .unwrap()
                .send_ui_event(UiFrontendEvent::Busy(BusyParams { busy }));
        });
    }

    /// Polls for changes to the working directory, and sends an event to the
    /// frontend if the working directory has changed.
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
            self.send_ui_event(UiFrontendEvent::WorkingDirectory(WorkingDirectoryParams {
                directory: current_dir.to_string_lossy().to_string(),
            }));
        };
        Ok(())
    }

    /// Check if the Positron frontend is connected
    pub fn ui_connected(&self) -> bool {
        self.ui_comm_tx.is_some()
    }

    /// Send events or requests to the frontend (Positron-specific)
    pub fn send_ui_event(&self, event: UiFrontendEvent) {
        self.send_ui(UiCommMessage::Event(event))
    }
    pub fn send_ui_request(&self, request: UiCommFrontendRequest) {
        self.send_ui(UiCommMessage::Request(request))
    }

    fn send_ui(&self, msg: UiCommMessage) {
        log::info!("Sending UI message to frontend: {msg:?}");

        if !self.ui_connected() {
            log::info!("Discarding message {msg:?}; no frontend UI comm connected");
            return;
        }

        let ui_comm_tx = self.ui_comm_tx.as_ref().unwrap();

        if let Err(err) = ui_comm_tx.send(msg) {
            log::error!("Error sending message to frontend UI comm: {err:?}");

            // TODO: Something is wrong with the UI thread, we should
            // disconnect to avoid more errors but then we need a mutable self
            // self.frontend_tx = None;
        }
    }
}
