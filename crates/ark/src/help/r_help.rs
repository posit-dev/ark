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
use anyhow::anyhow;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use log::info;
use log::trace;
use log::warn;
use stdext::spawn;

use crate::help::message::HelpEvent;
use crate::help::message::ShowHelpUrlParams;
use crate::r_task;

/**
 * The R Help handler (together with the help proxy) provides the server side of
 * Positron's Help panel.
 */
pub struct RHelp {
    comm: CommSocket,
    r_port: u16,
    proxy_port: u16,
    help_event_rx: Receiver<HelpEvent>,
}

impl RHelp {
    /**
     * Start the help handler. Returns a channel for sending help events to
     * the help thread.
     *
     * - `comm`: The socket for communicating with the frontend.
     * - `r_port`: The R help server port.
     * - `proxy_port`: Our proxy help server port.
     */
    pub fn start(
        comm: CommSocket,
        r_port: u16,
        proxy_port: u16,
    ) -> anyhow::Result<Sender<HelpEvent>> {
        // Create the channel that will be used to send help events from other threads.
        let (help_event_tx, help_event_rx) = crossbeam::channel::unbounded();

        // Start the help thread and wait for requests from the frontend or events
        // from another thread.
        spawn!("ark-help", move || {
            let help = Self {
                comm,
                r_port,
                proxy_port,
                help_event_rx,
            };

            help.execution_thread();
        });

        // Return the channel for sending help events to the help thread
        Ok(help_event_tx)
    }

    /// Public associated function so that callers of `start()` can cheaply check if
    /// a url is a help url without sending a message over the execution thread
    /// (like in the case of `browseURL()`).
    pub fn is_help_url(url: &str, port: u16) -> bool {
        let prefix = Self::help_url_prefix(port);
        url.starts_with(prefix.as_str())
    }

    fn help_url_prefix(port: u16) -> String {
        format!("http://127.0.0.1:{port}/")
    }

    /**
     * The main help execution thread; receives messages from the frontend and
     * other threads and processes them.
     */
    fn execution_thread(&self) {
        loop {
            // Wait for either a message from the frontend or a help event
            // from another thread.
            select! {
                // A message from the frontend; typically a request to show
                // help for a specific topic.
                recv(&self.comm.incoming_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if !self.handle_comm_message(msg) {
                                info!("Help comm {} closing by request from frontend.", self.comm.comm_id);
                                break;
                            }
                        },
                        Err(err) => {
                            // The connection with the frontend has been closed; let
                            // the thread exit.
                            warn!("Error receiving message from frontend: {:?}", err);
                            break;
                        },
                    }
                },

                // A message from another thread, typically notifying us that a
                // help URL is ready for viewing.
                recv(&self.help_event_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if let Err(err) = self.handle_event(msg) {
                                log::error!("Error handling Help event: {:?}", err);
                            }
                        },
                        Err(err) => {
                            // The connection with the frontend has been closed; let
                            // the thread exit.
                            log::error!("Error receiving internal Help message: {:?}", err);
                            break;
                        },
                    }
                },
            }
        }
        trace!("Help comm {} closed.", self.comm.comm_id);
    }

    /**
     * Handles a comm message from the frontend.
     *
     * Returns true if the thread should continue, false if it should exit.
     */
    fn handle_comm_message(&self, message: CommMsg) -> bool {
        if let CommMsg::Close = message {
            // The frontend has closed the connection; let the
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

    #[tracing::instrument(level = "trace", skip_all, fields(message = %message))]
    fn handle_event(&self, message: HelpEvent) -> anyhow::Result<()> {
        log::trace!("{message:#?}");
        match message {
            HelpEvent::ShowHelpUrl(params) => self.handle_show_help_url(params),
        }
    }

    /// Shows a help URL by sending a message to the frontend. We expect that any URL
    /// coming through here has already been verified to look like a help URL with
    /// `is_help_url()`, so if we get an unexpected prefix, that's an error.
    fn handle_show_help_url(&self, params: ShowHelpUrlParams) -> anyhow::Result<()> {
        let url = params.url;

        if !Self::is_help_url(url.as_str(), self.r_port) {
            let prefix = Self::help_url_prefix(self.r_port);
            return Err(anyhow!(
                "Help URL '{url}' doesn't have expected prefix '{prefix}'."
            ));
        }

        // Re-direct the help event to our help proxy server.
        let r_prefix = Self::help_url_prefix(self.r_port);
        let proxy_prefix = Self::help_url_prefix(self.proxy_port);

        let proxy_url = url.replace(r_prefix.as_str(), proxy_prefix.as_str());

        log::trace!(
            "Sending frontend event `ShowHelp` with R url '{url}' and proxy url '{proxy_url}'"
        );

        let msg = HelpFrontendEvent::ShowHelp(ShowHelpParams {
            content: proxy_url,
            kind: ShowHelpKind::Url,
            focus: true,
        });
        let json = serde_json::to_value(msg)?;
        self.comm.outgoing_tx.send(CommMsg::Data(json))?;

        // The URL was sent to the frontend.
        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn show_help_topic(&self, topic: String) -> anyhow::Result<bool> {
        let found = r_task(|| unsafe {
            RFunction::from(".ps.help.showHelpTopic")
                .add(topic)
                .call()?
                .to::<bool>()
        })?;
        Ok(found)
    }

    pub fn r_start_or_reconnect_to_help_server() -> harp::Result<u16> {
        // Start the R help server.
        // If it is already started, it just returns the preexisting port number.
        RFunction::from(".ps.help.startOrReconnectToHelpServer")
            .call()
            .and_then(|x| x.try_into())
    }
}
