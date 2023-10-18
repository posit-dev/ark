//
// r_help.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::socket::comm::CommSocket;
use anyhow::anyhow;
use anyhow::Result;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use log::error;
use log::info;
use log::trace;
use log::warn;
use stdext::spawn;

use crate::browser;
use crate::help::message::HelpMessage;
use crate::help::message::HelpRequest;
use crate::help::message::ShowHelpContent;
use crate::help::message::ShowTopicReply;
use crate::help_proxy;
use crate::r_task;

/**
 * The R Help handler (together with the help proxy) provides the server side of
 * Positron's Help panel.
 */
pub struct RHelp {
    comm: CommSocket,
    r_help_port: u16,
    help_request_rx: Receiver<HelpRequest>,
}

impl RHelp {
    /**
     * Start the help handler. Returns a channel for sending help requests to
     * the help thread.
     *
     * - `comm`: The socket for communicating with the front end.
     */
    pub fn start(comm: CommSocket) -> Result<Sender<HelpRequest>> {
        // Check to see whether the help server has started. We set the port
        // number when it starts, so if it's still at the default value (0), it
        // hasn't started.
        let mut started = false;
        let r_help_port: u16;
        unsafe {
            if browser::PORT != 0 {
                started = true;
            }
        }

        if started {
            // We have already started the help server; get the port number.
            r_help_port =
                r_task(|| unsafe { RFunction::new("tools", "httpdPort").call()?.to::<u16>() })?;
            trace!(
                "Help comm {} started; reconnected help server on port {}",
                comm.comm_id,
                r_help_port
            );
        } else {
            // If we haven't started the help server, start it now.
            r_help_port = RHelp::start_help_server()?;
            trace!(
                "Help comm {} started; started help server on port {}",
                comm.comm_id,
                r_help_port
            );
        }

        // Create the channels that will be used to communicate with the help
        // thread from other threads.
        let (help_request_tx, help_request_rx) = crossbeam::channel::unbounded();

        // Start the help request thread and wait for requests from the front
        // end.
        spawn!("ark-help", move || {
            let help = Self {
                comm,
                r_help_port,
                help_request_rx,
            };
            help.execution_thread();
        });

        // Return the channel for sending help requests to the help thread.
        Ok(help_request_tx)
    }

    /**
     * The main help execution thread; receives messages from the frontend and
     * other threads and processes them.
     */
    pub fn execution_thread(&self) {
        loop {
            // Wait for either a message from the front end or a help request
            // from another thread.
            select! {
                // A message from the front end; typically a request to show
                // help for a specific topic.
                recv(&self.comm.incoming_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if !self.handle_comm_message(msg) {
                                info!("Help comm {} closing by request from front end.", self.comm.comm_id);
                                break;
                            }
                        },
                        Err(err) => {
                            // The connection with the front end has been closed; let
                            // the thread exit.
                            warn!("Error receiving message from front end: {:?}", err);
                            break;
                        },
                    }
                },

                // A message from another thread, typically notifying us that a
                // help URL is ready for viewing.
                recv(&self.help_request_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if let Err(err) = self.handle_request(msg) {
                                warn!("Error handling Help request: {:?}", err);
                            }
                        },
                        Err(err) => {
                            // The connection with the front end has been closed; let
                            // the thread exit.
                            warn!("Error receiving internal Help message: {:?}", err);
                            break;
                        },
                    }
                },
            }
        }
        trace!("Help comm {} closed.", self.comm.comm_id);
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
                    error!("Help: Received invalid message from front end. {:?}", err);
                    return true;
                },
            };
            if let Err(err) = self.handle_message(id, message) {
                error!("Help: Error handling message from front end: {:?}", err);
                return true;
            }
        }

        true
    }

    fn handle_message(&self, id: String, message: HelpMessage) -> Result<()> {
        // Match on the type of data received.
        match message {
            HelpMessage::ShowHelpTopicRequest(topic) => {
                // Look up the help topic and attempt to show it; this returns a
                // boolean indicating whether the topic was found.
                let found = match self.show_help_topic(topic.topic.clone()) {
                    Ok(found) => found,
                    Err(err) => {
                        error!("Error looking up help topic {}: {:?}", topic.topic, err);
                        false
                    },
                };

                // Create and send a reply to the front end.
                let reply = HelpMessage::ShowHelpTopicReply(ShowTopicReply { found });
                let json = serde_json::to_value(reply)?;
                self.comm.outgoing_tx.send(CommChannelMsg::Rpc(id, json))?;
                Ok(())
            },
            _ => Err(anyhow!("Help: Received unexpected message {:?}", message)),
        }
    }

    fn handle_request(&self, message: HelpRequest) -> Result<()> {
        match message {
            HelpRequest::ShowHelpUrlRequest(url) => self.show_help_url(url.as_str()),
            _ => Err(anyhow!("Help: Received unexpected request {:?}", message)),
        }
    }

    fn show_help_url(&self, url: &str) -> Result<()> {
        // Check for help URLs
        let prefix = format!("http://127.0.0.1:{}/", self.r_help_port);
        if !url.starts_with(&prefix) {
            return Err(anyhow!(
                "Help URL '{}' doesn't have expected prefix '{}'",
                url,
                prefix
            ));
        }

        // Re-direct the help request to our help proxy server.
        let proxy_port = unsafe { browser::PORT };
        let replacement = format!("http://127.0.0.1:{}/", proxy_port);

        let url = url.replace(prefix.as_str(), replacement.as_str());
        let msg = HelpMessage::ShowHelpEvent(ShowHelpContent {
            content: url,
            kind: "url".to_string(),
            focus: true,
        });
        let json = serde_json::to_value(msg)?;
        self.comm.outgoing_tx.send(CommChannelMsg::Data(json))?;
        Ok(())
    }

    fn show_help_topic(&self, topic: String) -> Result<bool> {
        let found = r_task(|| unsafe {
            RFunction::from(".ps.help.showHelpTopic")
                .add(topic)
                .call()?
                .to::<bool>()
        })?;
        Ok(found)
    }

    fn start_help_server() -> Result<u16> {
        // Start the R side of the help server
        let help_server_port = r_task(|| unsafe {
            RFunction::from(".ps.help.startHelpServer")
                .call()?
                .to::<u16>()
        })?;

        // Start the help proxy server
        help_proxy::start(help_server_port);
        Ok(help_server_port)
    }
}
