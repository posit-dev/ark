//
// frontend.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::frontend_comm::FrontendBackendRpcReply;
use amalthea::comm::frontend_comm::FrontendBackendRpcRequest;
use amalthea::comm::frontend_comm::FrontendEvent;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::stdin::StdInRequest;
use amalthea::wire::input_request::CommRequest;
use crossbeam::channel::Receiver;
use crossbeam::channel::Select;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use serde_json::Value;
use stdext::spawn;

use crate::r_task;

#[derive(Debug)]
pub enum PositronFrontendMessage {
    Event(FrontendEvent),
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
        // We select dynamically so we can close event sources individually when
        // there is an error instead of shutting down the whole frontend thread
        let mut sel = Select::new();
        let main_idx = sel.recv(&self.frontend_rx);
        let comm_idx = sel.recv(&self.comm.incoming_rx);
        let mut n_ok = 2;

        let error_handler = |i, name, err, sel: &mut Select, n_ok: &mut i32| {
            log::error!(
                "Error receiving frontend message on {name} source; closing event listener\n{err:?}"
            );
            sel.remove(i);
            *n_ok = *n_ok - 1;
        };

        loop {
            // Close threads if all event sources have been closed
            if n_ok == 0 {
                break;
            }

            // Wait for an event on either the event channel (which forwards
            // Positron events or requests to the frontend) or the comm channel
            // (which receives requests from the frontend)
            let op = sel.select();
            match op.index() {
                i if i == main_idx => match op.recv(&self.frontend_rx) {
                    Ok(msg) => match msg {
                        PositronFrontendMessage::Event(event) => self.dispatch_event(&event),
                        PositronFrontendMessage::Request(request) => {
                            self.call_frontend_method(request).unwrap()
                        },
                    },
                    Err(err) => error_handler(i, "internal", err, &mut sel, &mut n_ok),
                },

                i if i == comm_idx => match op.recv(&self.comm.incoming_rx) {
                    Ok(msg) => {
                        if !self.handle_comm_message(msg) {
                            log::info!(
                                "Frontend comm {} closing by request from front end.",
                                self.comm.comm_id
                            );
                            break;
                        }
                    },
                    Err(err) => error_handler(i, "external", err, &mut sel, &mut n_ok),
                },

                _ => unreachable!(),
            }
        }
    }

    fn dispatch_event(&self, event: &FrontendEvent) {
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
        request: FrontendBackendRpcRequest,
    ) -> anyhow::Result<FrontendBackendRpcReply, anyhow::Error> {
        let request = match request {
            FrontendBackendRpcRequest::CallMethod(request) => request,
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

        Ok(FrontendBackendRpcReply::CallMethodReply(result))
    }

    fn call_frontend_method(&self, request: CommRequest) -> anyhow::Result<()> {
        let comm_msg = StdInRequest::Comm(request);
        self.stdin_request_tx.send(comm_msg)?;

        Ok(())
    }
}
