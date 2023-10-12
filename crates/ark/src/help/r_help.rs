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
use harp::exec::RFunction;
use log::error;
use log::warn;
use stdext::spawn;

use crate::help::message::HelpMessage;
use crate::help_proxy;
use crate::r_task;

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
        // Get the help port from R and start the help proxy thread.
        //
        // CONSIDER: What happens on a UI reload? In this case the help comm
        // will get shut down, but the help proxy will keep on running, and then
        // we'll try to start another one here. That isn't good.
        let help_server_port =
            r_task(|| unsafe { RFunction::new("tools", "httpdPort").call()?.to::<u16>() });

        match help_server_port {
            Ok(port) => {
                // Start the help proxy.
                help_proxy::start(port);
            },
            Err(err) => {
                error!(
                    "Help: Error getting help server port from R: {}; not starting help proxy.",
                    err
                );
                return;
            },
        }

        // Start the help request thread and wait for requests from the front end
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
                        self.handle_message(message);
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

    fn handle_message(&self, message: HelpMessage) {
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
}
