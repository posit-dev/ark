//
// r_help.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::help_comm::HelpBackendReply;
use amalthea::comm::help_comm::HelpBackendRequest;
use amalthea::comm::help_comm::HelpFrontendEvent;
use amalthea::comm::help_comm::ShowHelpKind;
use amalthea::comm::help_comm::ShowHelpParams;
use amalthea::socket::comm::CommSocket;
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
use crate::help::message::HelpReply;
use crate::help::message::HelpRequest;
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
    help_reply_tx: Sender<HelpReply>,
}

impl RHelp {
    /**
     * Start the help handler. Returns a channel for sending help requests to
     * the help thread.
     *
     * - `comm`: The socket for communicating with the front end.
     */
    pub fn start(comm: CommSocket) -> Result<(Sender<HelpRequest>, Receiver<HelpReply>)> {
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
        let (help_reply_tx, help_reply_rx) = crossbeam::channel::unbounded();

        // Start the help request thread and wait for requests from the front
        // end.
        spawn!("ark-help", move || {
            let help = Self {
                comm,
                r_help_port,
                help_request_rx,
                help_reply_tx,
            };

            help.execution_thread();
        });

        // Return the channel for sending help requests to the help thread.
        Ok((help_request_tx, help_reply_rx))
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
    fn handle_comm_message(&self, message: CommMsg) -> bool {
        if let CommMsg::Close = message {
            // The front end has closed the connection; let the
            // thread exit.
            return false;
        }

        if self
            .comm
            .handle_request(message, |req| self.handle_rpc(req))
        {
            return true;
        }

        true
    }

    fn handle_rpc(&self, message: HelpBackendRequest) -> anyhow::Result<HelpBackendReply> {
        // Match on the type of data received.
        match message {
            HelpBackendRequest::ShowHelpTopic(topic) => {
                // Look up the help topic and attempt to show it; this returns a
                // boolean indicating whether the topic was found.
                match self.show_help_topic(topic.topic.clone()) {
                    Ok(found) => Ok(HelpBackendReply::ShowHelpTopicReply(found)),
                    Err(err) => Err(err),
                }
            },
        }
    }

    fn handle_request(&self, message: HelpRequest) -> Result<()> {
        match message {
            HelpRequest::ShowHelpUrlRequest(url) => {
                let found = match self.show_help_url(&url) {
                    Ok(found) => found,
                    Err(err) => {
                        error!("Error showing help URL {}: {:?}", url, err);
                        false
                    },
                };
                self.help_reply_tx
                    .send(HelpReply::ShowHelpUrlReply(found))?;
            },
        }
        Ok(())
    }

    /// Shows a help URL by sending a message to the front end. Returns
    /// `Ok(true)` if the URL was handled, `Ok(false)` if it wasn't.
    fn show_help_url(&self, url: &str) -> Result<bool> {
        // Check for help URLs. If this is an R help URL, we'll re-direct it to
        // our help proxy server.
        let prefix = format!("http://127.0.0.1:{}/", self.r_help_port);
        if !url.starts_with(&prefix) {
            info!(
                "Help URL '{}' doesn't have expected prefix '{}'; not handling",
                url, prefix
            );
            return Ok(false);
        }

        // Re-direct the help request to our help proxy server.
        let proxy_port = unsafe { browser::PORT };
        let replacement = format!("http://127.0.0.1:{}/", proxy_port);

        let url = url.replace(prefix.as_str(), replacement.as_str());
        let msg = HelpFrontendEvent::ShowHelp(ShowHelpParams {
            content: url,
            kind: ShowHelpKind::Url,
            focus: true,
        });
        let json = serde_json::to_value(msg)?;
        self.comm.outgoing_tx.send(CommMsg::Data(json))?;

        // The URL was handled.
        Ok(true)
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
