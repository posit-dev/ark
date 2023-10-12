//
// r_help.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::exec::RFunction;
use log::error;
use log::warn;
use stdext::spawn;

use crate::browser;
use crate::help::message::HelpMessage;
use crate::help::message::HelpRequest;
use crate::help_proxy;
use crate::r_task;

/**
 * The R Help handler (together with the help proxy) provides the server side of
 * Positron's Help panel.
 */
pub struct RHelp {
    comm: CommSocket,
    help_request_rx: Receiver<HelpRequest>,
}

impl RHelp {
    pub fn start(comm: CommSocket) -> Sender<HelpRequest> {
        // Check to see whether the help server has started. We set the port
        // number when it starts, so if it's still at the default value (0), it
        // hasn't started.
        let mut started = false;
        unsafe {
            if browser::PORT != 0 {
                started = true;
            }
        }

        // If we haven't started the help server, start it now.
        if !started {
            RHelp::start_help_proxy();
        }

        let (help_request_tx, help_request_rx) = crossbeam::channel::unbounded();

        // Start the help request thread and wait for requests from the front end
        spawn!("ark-help", move || {
            let help = Self {
                comm,
                help_request_rx,
            };
            help.execution_thread();
        });

        help_request_tx
    }

    pub fn execution_thread(&self) {
        loop {
            select! {
                recv(&self.comm.incoming_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if !self.handle_comm_message(msg) {
                                break;
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
    }

    /**
     * Handles a comm message from the front end.
     *
     * Returns true if the thread should continue, false if it should exit.
     */
    fn handle_comm_message(&self, message: CommChannelMsg) -> bool {
        if let CommChannelMsg::Close = message {
            // The front end has closed the connection; let the
            // thread exit.
            return false;
        }
        if let CommChannelMsg::Rpc(id, data) = message {
            let message = match serde_json::from_value::<HelpMessage>(data) {
                Ok(m) => m,
                Err(err) => {
                    error!("Help: Received invalid message from front end. {}", err);
                    return true;
                },
            };
            self.handle_message(message);
        }

        true
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

    fn start_help_proxy() {
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
    }
}
