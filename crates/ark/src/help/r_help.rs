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
use log::error;
use log::warn;
use stdext::spawn;

use crate::browser;
use crate::help::message::HelpMessage;
use crate::help::message::HelpMessageShowHelp;
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
    help_proxy_port: Option<u16>,
    help_request_rx: Receiver<HelpRequest>,
}

impl RHelp {
    pub fn start(comm: CommSocket) -> Result<Sender<HelpRequest>> {
        // Check to see whether the help server has started. We set the port
        // number when it starts, so if it's still at the default value (0), it
        // hasn't started.
        let mut started = false;
        let mut r_help_port = 0;
        unsafe {
            if browser::PORT != 0 {
                started = true;
                r_help_port = browser::PORT;
            }
        }

        // If we haven't started the help server, start it now.
        if !started {
            r_help_port = RHelp::start_help_server()?
        }

        let (help_request_tx, help_request_rx) = crossbeam::channel::unbounded();

        // Start the help request thread and wait for requests from the front end
        spawn!("ark-help", move || {
            let help = Self {
                comm,
                r_help_port,
                help_proxy_port: None,
                help_request_rx,
            };
            help.execution_thread();
        });

        Ok(help_request_tx)
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
                },
                recv(&self.help_request_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            self.handle_request(msg);
                        },
                        Err(e) => {
                            // The connection with the front end has been closed; let
                            // the thread exit.
                            warn!("Error receiving internal Help message: {:?}", e);
                            break;
                        },
                    }
                },
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

    fn handle_request(&self, message: HelpRequest) -> Result<()> {
        match message {
            HelpRequest::SetHelpProxyPort(port) => unsafe {
                browser::PORT = port;
            },
            HelpRequest::ShowHelpUrl(url) => {
                self.show_help_url(url.as_str())?;
            },
        }
        Ok(())
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
        let replacement = format!("http://127.0.0.1:{}/", self.r_help_port);

        // TODO: Fire an event for the front-end.
        let url = url.replace(prefix.as_str(), replacement.as_str());
        let msg = HelpMessage::ShowHelp(HelpMessageShowHelp {
            content: url,
            kind: "url".to_string(),
            focus: true,
        });
        let json = serde_json::to_value(msg)?;
        self.comm.outgoing_tx.send(CommChannelMsg::Data(json))?;
        Ok(())
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
