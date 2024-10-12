use std::path::PathBuf;

use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::comm::ui_comm::WorkingDirectoryParams;
use amalthea::wire::input_request::UiCommFrontendRequest;
use crossbeam::channel::Sender;

use crate::ui::UiCommMessage;

pub struct UIComm {
    tx: Sender<UiCommMessage>,
    working_directory: PathBuf,
}

impl UIComm {
    pub fn new(tx: Sender<UiCommMessage>) -> Self {
        // Empty path buf will get updated on first directory refresh
        let working_directory = PathBuf::new();

        Self {
            tx,
            working_directory,
        }
    }

    pub fn send_event(&self, event: UiFrontendEvent) {
        self.send(UiCommMessage::Event(event))
    }

    pub fn send_request(&self, request: UiCommFrontendRequest) {
        self.send(UiCommMessage::Request(request))
    }

    fn send(&self, msg: UiCommMessage) {
        log::info!("Sending UI message to frontend: {msg:?}");

        if let Err(err) = self.tx.send(msg) {
            log::error!("Error sending message to frontend UI comm: {err:?}");

            // TODO: Something is wrong with the UI thread, we should
            // disconnect to avoid more errors but then we need a mutable self
            // self.frontend_tx = None;
        }
    }

    /// Checks for changes to the working directory, and sends an event to the
    /// frontend if the working directory has changed.
    pub fn refresh_working_directory(&mut self) -> anyhow::Result<()> {
        // Get the current working directory
        let mut new_working_directory = std::env::current_dir()?;

        // If it isn't the same as the last working directory, send an event
        if new_working_directory != self.working_directory {
            self.working_directory = new_working_directory.clone();

            // Attempt to alias the directory, if it's within the home directory
            if let Some(home_dir) = home::home_dir() {
                if let Ok(stripped_dir) = new_working_directory.strip_prefix(home_dir) {
                    let mut new_path = PathBuf::from("~");
                    new_path.push(stripped_dir);
                    new_working_directory = new_path;
                }
            }

            // Deliver event to client
            self.send_event(UiFrontendEvent::WorkingDirectory(WorkingDirectoryParams {
                directory: new_working_directory.to_string_lossy().to_string(),
            }));
        };

        Ok(())
    }
}
