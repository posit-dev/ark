//
// r_help.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Sender;
use log::error;
use log::warn;
use stdext::spawn;

use crate::help::message::HelpMessage;

/**
 * The R Help handler (together with the help proxy) provides the server side of
 * Positron's Help panel.
 */
pub struct RHelp {
    comm: CommSocket,
    comm_manager_tx: Sender<CommEvent>,
}

impl RHelp {
    pub fn start(comm: CommSocket, comm_manager_tx: Sender<CommEvent>) {
        // Start the help thread and wait for requests from the front end
        spawn!("ark-help", move || {
            let help = Self {
                comm,
                comm_manager_tx,
            };
            help.execution_thread();
        });
    }

    pub fn execution_thread(&self) {
        loop {
            match self.comm.incoming_rx.recv() {
                Ok(msg) => {
                    if let CommChannelMsg::Close = msg {
                        // The front end has closed the connection; let the
                        // thread exit.
                        break;
                    }
                    if let CommChannelMsg::Rpc(id, data) = msg {
                        let message = match serde_json::from_value::<HelpMessage>(data) {
                            Ok(m) => m,
                            Err(err) => {
                                error!("Help: Received invalid message from front end. {}", err);
                                continue;
                            },
                        };

                        // Match on the type of data received.
                        match message {
                            HelpMessage::ShowHelpTopic(topic) => {
                                // TODO: show the help topic
                            },
                            _ => {
                                warn!(
                                    "Help: Received unexpected message from front end: {:?}",
                                    message
                                );
                            },
                        }
                    }
                },
                Err(e) => {
                    // The connection with the front end has been closed; let
                    // the thread exit.
                    warn!("Error receiving message from front end: {}", e);
                    break;
                },
            }
        }
    }
}
