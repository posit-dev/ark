//
// frontend.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::ui_comm::UiBackendReply;
use amalthea::comm::ui_comm::UiBackendRequest;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::stdin::StdInRequest;
use amalthea::wire::input_request::CommRequest;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use serde_json::Value;
use stdext::spawn;
use stdext::unwrap;

use crate::r_task;

#[derive(Debug)]
pub enum PositronFrontendMessage {
    Event(UiFrontendEvent),
    Request(CommRequest),
}

/// PositronFrontend is a wrapper around a comm channel whose lifetime matches
/// that of the Positron front end. It is used to perform communication with the
/// front end that isn't scoped to any particular view.
pub struct PositronFrontend {
    comm: CommSocket,
    frontend_rx: Receiver<PositronFrontendMessage>,
    stdin_request_tx: Sender<StdInRequest>,
}

impl PositronFrontend {
    pub fn start(
        comm: CommSocket,
        stdin_request_tx: Sender<StdInRequest>,
    ) -> Sender<PositronFrontendMessage> {
        // Create a sender-receiver pair for Positron global events
        let (frontend_tx, frontend_rx) = crossbeam::channel::unbounded::<PositronFrontendMessage>();

        spawn!("ark-comm-frontend", move || {
            let frontend = Self {
                comm: comm.clone(),
                frontend_rx: frontend_rx.clone(),
                stdin_request_tx: stdin_request_tx.clone(),
            };
            frontend.execution_thread();
        });

        frontend_tx
    }

    fn execution_thread(&self) {
        loop {
            // Wait for an event on either the event channel (which forwards
            // Positron events to the frontend) or the comm channel (which
            // receives requests from the frontend)
            select! {
                recv(&self.frontend_rx) -> msg => {
                    let msg = unwrap!(msg, Err(err) => {
                        log::error!(
                            "Error receiving Positron event; closing event listener: {err:?}"
                        );
                        // Most likely the channel was closed, so we should stop the thread
                        break;
                    });
                    match msg {
                        PositronFrontendMessage::Event(event) => self.dispatch_event(&event),
                        PositronFrontendMessage::Request(request) => self.call_frontend_method(request).unwrap(),
                    }
                },

                recv(&self.comm.incoming_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if !self.handle_comm_message(msg) {
                                log::info!("Frontend comm {} closing by request from front end.", self.comm.comm_id);
                                break;
                            }
                        },
                        Err(err) => {
                            log::error!("Error receiving message from front end: {:?}", err);
                            break;
                        },
                    }
                },
            }
        }
    }

    fn dispatch_event(&self, event: &UiFrontendEvent) {
        let json = serde_json::to_value(event).unwrap();

        // Deliver the event to the front end over the comm channel
        if let Err(err) = self.comm.outgoing_tx.send(CommMsg::Data(json)) {
            log::error!("Error sending Positron event to front end: {}", err);
        };
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
            .handle_request(message.clone(), |req| self.handle_rpc(req))
        {
            return true;
        }

        // We don't really expect to receive data messages from the
        // front end; they are events
        log::warn!("Unexpected data message from front end: {message:?}");
        true
    }

    /**
     * Handles an RPC request from the front end.
     */
    fn handle_rpc(
        &self,
        request: UiBackendRequest,
    ) -> anyhow::Result<UiBackendReply, anyhow::Error> {
        let request = match request {
            UiBackendRequest::CallMethod(request) => request,
        };

        log::trace!("Handling '{}' frontend RPC method", request.method);

        // Today, all RPCs are fulfilled by R directly. Check to see if an R
        // method of the appropriate name is defined.
        //
        // Consider: In the future, we may want to allow requests to be
        // fulfilled here on the Rust side, with only some requests forwarded to
        // R; Rust methods may wish to establish their own RPC handlers.

        // The method name is prefixed with ".ps.rpc.", by convention
        let method = format!(".ps.rpc.{}", request.method);

        // Use the `exists` function to see if the method exists
        let exists = r_task(|| unsafe {
            let exists = RFunction::from("exists")
                .param("x", method.clone())
                .call()?;
            RObject::to::<bool>(exists)
        })?;

        if !exists {
            anyhow::bail!("No such method: {}", request.method);
        }

        // Form an R function call from the request
        let result = r_task(|| {
            let mut call = RFunction::from(method);
            for param in request.params.iter() {
                let p = RObject::try_from(param.clone())?;
                call.add(p);
            }
            let result = call.call()?;
            Value::try_from(result)
        })?;

        Ok(UiBackendReply::CallMethodReply(result))
    }

    fn call_frontend_method(&self, request: CommRequest) -> anyhow::Result<()> {
        let comm_msg = StdInRequest::Comm(request);
        self.stdin_request_tx.send(comm_msg)?;

        Ok(())
    }
}
