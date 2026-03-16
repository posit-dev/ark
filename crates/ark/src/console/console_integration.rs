//
// console_integration.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

//! Help, LSP, UI comm, and frontend method integration for the R console.

use super::*;

/// UI comm integration.
impl Console {
    pub(super) fn handle_establish_ui_comm_channel(
        &mut self,
        ui_comm_tx: Sender<UiCommMessage>,
        info: &PromptInfo,
    ) {
        if self.ui_comm_tx.is_some() {
            log::info!("Replacing an existing UI comm channel.");
        }

        // Create and store the sender channel
        self.ui_comm_tx = Some(UiCommSender::new(ui_comm_tx));

        // Go ahead and do an initial refresh
        self.with_mut_ui_comm_tx(|ui_comm_tx| {
            let input_prompt = info.input_prompt.clone();
            let continuation_prompt = info.continuation_prompt.clone();

            ui_comm_tx.send_refresh(input_prompt, continuation_prompt);
        });
    }

    pub(crate) fn session_mode(&self) -> SessionMode {
        self.session_mode
    }

    pub(crate) fn get_ui_comm_tx(&self) -> Option<&UiCommSender> {
        self.ui_comm_tx.as_ref()
    }

    fn get_mut_ui_comm_tx(&mut self) -> Option<&mut UiCommSender> {
        self.ui_comm_tx.as_mut()
    }

    pub(super) fn with_ui_comm_tx<F>(&self, f: F)
    where
        F: FnOnce(&UiCommSender),
    {
        match self.get_ui_comm_tx() {
            Some(ui_comm_tx) => f(ui_comm_tx),
            None => {
                // Trace level logging, its typically not a bug if the frontend
                // isn't connected. Happens in all Jupyter use cases.
                log::trace!("UI comm isn't connected, dropping `f`.");
            },
        }
    }

    pub(super) fn with_mut_ui_comm_tx<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut UiCommSender),
    {
        match self.get_mut_ui_comm_tx() {
            Some(ui_comm_tx) => f(ui_comm_tx),
            None => {
                // Trace level logging, its typically not a bug if the frontend
                // isn't connected. Happens in all Jupyter use cases.
                log::trace!("UI comm isn't connected, dropping `f`.");
            },
        }
    }

    pub(crate) fn is_ui_comm_connected(&self) -> bool {
        self.get_ui_comm_tx().is_some()
    }

    pub(crate) fn call_frontend_method(
        &self,
        request: UiFrontendRequest,
    ) -> anyhow::Result<RObject> {
        log::trace!("Calling frontend method {request:?}");

        let ui_comm_tx = self.get_ui_comm_tx().ok_or_else(|| {
            anyhow::anyhow!("UI comm is not connected. Can't execute request {request:?}")
        })?;

        let (reply_tx, reply_rx) = bounded(1);

        let Some(req) = &self.active_request else {
            return Err(anyhow::anyhow!(
                "No active request. Can't execute request {request:?}"
            ));
        };

        // Forward request to UI comm
        ui_comm_tx.send_request(UiCommFrontendRequest {
            originator: req.originator.clone(),
            reply_tx,
            request: request.clone(),
        });

        // Block for reply
        let reply = reply_rx.recv().unwrap();

        log::trace!("Got reply from frontend method: {reply:?}");

        match reply {
            StdInRpcReply::Reply(reply) => match reply {
                JsonRpcReply::Result(reply) => {
                    // Deserialize to Rust first to verify the OpenRPC contract.
                    // Errors are propagated to R.
                    if let Err(err) = ui_frontend_reply_from_value(reply.result.clone(), &request) {
                        return Err(anyhow::anyhow!(
                            "Can't deserialize RPC reply for {request:?}:\n{err:?}"
                        ));
                    }

                    // Now deserialize to an R object
                    Ok(RObject::try_from(reply.result)?)
                },
                JsonRpcReply::Error(reply) => {
                    let message = reply.error.message;

                    Err(anyhow::anyhow!(
                        "While calling frontend method:\n\
                         {message}",
                    ))
                },
            },
            // If an interrupt was signalled, return `NULL`. This should not be
            // visible to the caller since `r_unwrap()` (called e.g. by
            // `harp::register`) will trigger an interrupt jump right away.
            StdInRpcReply::Interrupt => Ok(RObject::null()),
        }
    }
}

/// Help integration.
impl Console {
    pub(crate) fn set_help_fields(&mut self, help_event_tx: Sender<HelpEvent>, help_port: u16) {
        self.help_event_tx = Some(help_event_tx);
        self.help_port = Some(help_port);
    }

    pub(crate) fn send_help_event(&self, event: HelpEvent) -> anyhow::Result<()> {
        let Some(ref tx) = self.help_event_tx else {
            return Err(anyhow!("No help channel available to handle help event. Is the help comm open? Event {event:?}."));
        };

        if let Err(err) = tx.send(event) {
            return Err(anyhow!("Failed to send help message: {err:?}"));
        }

        Ok(())
    }

    pub(crate) fn is_help_url(&self, url: &str) -> bool {
        let Some(port) = self.help_port else {
            log::error!("No help port is available to check if '{url}' is a help url. Is the help comm open?");
            // Fail to recognize this as a help url, allow any fallbacks methods to run instead.
            return false;
        };

        RHelp::is_help_url(url, port)
    }
}

/// LSP integration.
impl Console {
    fn send_lsp_notification(&mut self, event: KernelNotification) {
        log::trace!(
            "Sending LSP notification: {event:#?}",
            event = event.trace()
        );

        let Some(ref tx) = self.lsp_events_tx else {
            log::trace!("Failed to send LSP notification. LSP events channel isn't open yet, or has been closed. Event: {event:?}", event = event.trace());
            return;
        };

        if let Err(err) = tx.send(Event::Kernel(event)) {
            log::error!(
                "Failed to send LSP notification. Removing LSP events channel. Error: {err:?}"
            );
            self.remove_lsp_channel();
        }
    }

    pub(crate) fn set_lsp_channel(&mut self, lsp_events_tx: TokioUnboundedSender<Event>) {
        self.lsp_events_tx = Some(lsp_events_tx.clone());

        // Refresh LSP state now since we probably have missed some updates
        // while the channel was offline. This is currently not an ideal timing
        // as the channel is set up from a preemptive `r_task()` after the LSP
        // is set up. We'll want to do this in an idle task.
        log::trace!("LSP channel opened. Refreshing state.");
        self.refresh_lsp();
        self.notify_lsp_of_known_virtual_documents();
    }

    pub(crate) fn remove_lsp_channel(&mut self) {
        self.lsp_events_tx = None;
    }

    pub(super) fn refresh_lsp(&mut self) {
        match console_inputs() {
            Ok(inputs) => {
                self.send_lsp_notification(KernelNotification::DidChangeConsoleInputs(inputs));
            },
            Err(err) => log::error!("Can't retrieve console inputs: {err:?}"),
        }
    }
}

/// Virtual document integration.
impl Console {
    fn notify_lsp_of_known_virtual_documents(&mut self) {
        // Clone the whole HashMap since we need to own the uri/contents to send them
        // over anyways. We don't want to clear the map in case the LSP restarts later on
        // and we need to send them over again.
        let virtual_documents = self.lsp_virtual_documents.clone();

        for (uri, contents) in virtual_documents {
            self.send_lsp_notification(KernelNotification::DidOpenVirtualDocument(
                DidOpenVirtualDocumentParams { uri, contents },
            ))
        }
    }

    pub(crate) fn insert_virtual_document(&mut self, uri: String, contents: String) {
        log::trace!("Inserting vdoc for `{uri}`");

        // Strip scheme if any. We're only storing the path.
        let uri = uri.strip_prefix("ark:").unwrap_or(&uri).to_string();

        // Save our own copy of the virtual document. If the LSP is currently closed
        // or restarts, we can notify it of all virtual documents it should know about
        // in the LSP channel setup step. It is common for the kernel to create the
        // virtual documents for base R packages before the LSP has started up.
        self.lsp_virtual_documents
            .insert(uri.clone(), contents.clone());

        self.send_lsp_notification(KernelNotification::DidOpenVirtualDocument(
            DidOpenVirtualDocumentParams { uri, contents },
        ))
    }

    pub(super) fn remove_virtual_document(&mut self, uri: String) {
        log::trace!("Removing vdoc for `{uri}`");

        // Strip scheme if any. We're only storing the path.
        let uri = uri.strip_prefix("ark:").unwrap_or(&uri).to_string();

        self.lsp_virtual_documents.remove(&uri);

        self.send_lsp_notification(KernelNotification::DidCloseVirtualDocument(
            DidCloseVirtualDocumentParams { uri },
        ))
    }

    pub(crate) fn has_virtual_document(&self, uri: &str) -> bool {
        let uri = uri.strip_prefix("ark:").unwrap_or(uri).to_string();
        self.lsp_virtual_documents.contains_key(&uri)
    }

    pub(crate) fn get_virtual_document(&self, uri: &str) -> Option<String> {
        let uri = uri.strip_prefix("ark:").unwrap_or(uri);
        self.lsp_virtual_documents.get(uri).cloned()
    }
}
