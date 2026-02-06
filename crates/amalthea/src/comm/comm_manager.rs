/*
 * comm_manager.rs
 *
 * Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use log::info;
use log::warn;
use stdext::result::ResultExt;
use stdext::spawn;

use crate::comm::comm_channel::CommMsg;
use crate::comm::event::CommInfo;
use crate::comm::event::CommManagerEvent;
use crate::comm::event::CommManagerInfoReply;
use crate::comm::event::CommManagerRequest;
use crate::socket::comm::CommInitiator;
use crate::socket::comm::CommSocket;
use crate::socket::iopub::IOPubMessage;
use crate::wire::comm_open::CommOpen;

pub struct CommManager {
    open_comms: Vec<CommSocket>,
    iopub_tx: Sender<IOPubMessage>,
    comm_event_rx: Receiver<CommManagerEvent>,
}

impl CommManager {
    /**
     * The comm manager is responsible for listening for messages on all of the
     * open comms, attaching appropriate metadata, and relaying them to the front
     * end. It is meant to be called on a dedicated thread, and it does not return.
     *
     * - `iopub_tx`: The channel to send messages to the frontend.
     * - `comm_event_rx`: The channel to receive messages about changes to the set
     *   (or state) of open comms.
     */
    pub fn start(iopub_tx: Sender<IOPubMessage>, comm_event_rx: Receiver<CommManagerEvent>) {
        spawn!("comm-manager", move || {
            let mut comm_manager = CommManager::new(iopub_tx, comm_event_rx);
            loop {
                comm_manager.execution_thread();
            }
        });
    }

    /**
     * Create a new CommManager.
     */
    pub fn new(iopub_tx: Sender<IOPubMessage>, comm_event_rx: Receiver<CommManagerEvent>) -> Self {
        Self {
            iopub_tx,
            comm_event_rx,
            open_comms: Vec::<CommSocket>::new(),
        }
    }

    /**
     * The main execution thread for the comm manager; listens for comm events
     * and dispatches them accordingly. Blocks until a message is received;
     * intended to be called in a loop.
     *
     * NOTE: Comms now route their outgoing messages directly through IOPub
     * via CommOutgoingTx, so we no longer need to poll outgoing_rx from each
     * CommSocket. This thread just handles lifecycle events.
     */
    pub fn execution_thread(&mut self) {
        // Wait for a comm event (blocking call)
        let comm_event = match self.comm_event_rx.recv() {
            Ok(event) => event,
            Err(err) => {
                warn!("Error receiving comm_event message: {err}");
                return;
            },
        };

        match comm_event {
            // A Comm was opened
            CommManagerEvent::Opened(comm_socket, val) => {
                // Notify the frontend, if this request originated from the back end
                if comm_socket.initiator == CommInitiator::BackEnd {
                    self.iopub_tx
                        .send(IOPubMessage::CommOpen(CommOpen {
                            comm_id: comm_socket.comm_id.clone(),
                            target_name: comm_socket.comm_name.clone(),
                            data: val,
                        }))
                        .log_err();
                }

                // Add to our own list of open comms
                self.open_comms.push(comm_socket);

                info!(
                    "Comm channel opened; there are now {} open comms",
                    self.open_comms.len()
                );
            },

            // A message was received from the frontend
            CommManagerEvent::Message(comm_id, msg) => {
                // Find the index of the comm in the vector
                let index = self
                    .open_comms
                    .iter()
                    .position(|comm_socket| comm_socket.comm_id == comm_id);

                // If we found it, send the message to the comm
                if let Some(index) = index {
                    let comm = &self.open_comms[index];
                    log::trace!("Comm manager: Sending message to comm '{}'", comm.comm_name);

                    comm.incoming_tx.send(msg).log_err();
                } else {
                    log::warn!("Received message for unknown comm channel {comm_id}: {msg:?}",);
                }
            },

            // A Comm was closed; attempt to remove it from the set of open comms
            CommManagerEvent::Closed(comm_id) => {
                // Find the index of the comm in the vector
                let index = self
                    .open_comms
                    .iter()
                    .position(|comm_socket| comm_socket.comm_id == comm_id);

                // If we found it, remove it.
                if let Some(index) = index {
                    // Notify the comm that it's been closed
                    let comm = &self.open_comms[index];
                    comm.incoming_tx.send(CommMsg::Close).log_err();

                    // Remove it from our list of open comms
                    self.open_comms.remove(index);

                    info!(
                        "Comm channel closed; there are now {} open comms",
                        self.open_comms.len()
                    );
                } else {
                    warn!("Received close message for unknown comm channel {comm_id}",);
                }
            },

            // A comm manager request
            CommManagerEvent::Request(req) => match req {
                // Requesting information about the open comms
                CommManagerRequest::Info(tx) => {
                    let comms: Vec<CommInfo> = self
                        .open_comms
                        .iter()
                        .map(|comm| CommInfo {
                            id: comm.comm_id.clone(),
                            name: comm.comm_name.clone(),
                        })
                        .collect();

                    tx.send(CommManagerInfoReply { comms }).log_err();
                },
            },
        }
    }
}
