//
// frontend.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::frontend_comm::FrontendEvent;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
// use harp::exec::RFunction;
// use harp::exec::RFunctionExt;
// use harp::object::RObject;
use log::info;
// use serde_json::Value;
use stdext::spawn;

// use crate::r_task;

/// PositronFrontend is a wrapper around a comm channel whose lifetime matches
/// that of the Positron front end. It is used to perform communication with the
/// front end that isn't scoped to any particular view.
pub struct PositronFrontend {
    comm: CommSocket,
    event_rx: Receiver<FrontendEvent>,
}

impl PositronFrontend {
    pub fn start(comm: CommSocket) -> Sender<FrontendEvent> {
        // Create a sender-receiver pair for Positron global events
        let (event_tx, event_rx) = crossbeam::channel::unbounded::<FrontendEvent>();

        spawn!("ark-comm-frontend", move || {
            let frontend = Self {
                comm: comm.clone(),
                event_rx: event_rx.clone(),
            };
            frontend.execution_thread();
        });

        event_tx
    }

    fn execution_thread(&self) {
        loop {
            // Wait for an event on either the event channel (which forwards
            // Positron events to the frontend) or the comm channel (which
            // receives requests from the frontend)
            select! {
                recv(&self.event_rx) -> event => {
                    match event {
                        Ok(event) => self.dispatch_event(&event),
                        Err(err) => {
                            log::error!(
                                "Error receiving Positron event; closing event listener: {err:?}"
                            );
                            // Most likely the channel was closed, so we should stop the thread
                            break;
                        },
                    }
                },
                recv(&self.comm.incoming_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if !self.handle_comm_message(&msg) {
                                info!("Frontend comm {} closing by request from front end.", self.comm.comm_id);
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
    fn handle_comm_message(&self, msg: &CommMsg) -> bool {
        match msg {
            CommMsg::Data(data) => {
                // We don't really expect to receive data messages from the
                // front end; they are events
                log::warn!("Unexpected data message from front end: {:?}", data);
                true
            },
            CommMsg::Close => {
                // The front end has closed the connection; let the
                // thread exit.
                false
            },
            CommMsg::Rpc(_id, _request) => {
                todo!();
                // let message = match serde_json::from_value::<FrontendMessage>(request.clone()) {
                //     Ok(msg) => msg,
                //     Err(err) => {
                //         log::warn!("Error decoding RPC request from front end: {:?}", err);
                //         return true;
                //     },
                // };
                // match message {
                //     FrontendMessage::RpcRequest(req) => {
                //         if let Err(err) = self.handle_rpc_request(id, &req) {
                //             log::warn!("Error handling RPC request from front end: {:?}", err);
                //         }
                //     },
                //     _ => {
                //         log::warn!("Unexpected RPC message from front end: {:?}", message);
                //     },
                // };
                // true
            },
        }
    }

    // /**
    //  * Handles an RPC request from the front end.
    //  */
    // fn handle_rpc_request(
    //     &self,
    //     id: &str,
    //     request: &FrontendRpcRequest,
    // ) -> Result<(), anyhow::Error> {
    //     // Today, all RPCs are fulfilled by R directly. Check to see if an R
    //     // method of the appropriate name is defined.
    //     //
    //     // Consider: In the future, we may want to allow requests to be
    //     // fulfilled here on the Rust side, with only some requests forwarded to
    //     // R; Rust methods may wish to establish their own RPC handlers.

    //     // The method name is prefixed with ".ps.rpc.", by convention
    //     let method = format!(".ps.rpc.{}", request.method);

    //     // Use the `exists` function to see if the method exists
    //     let exists = r_task(|| unsafe {
    //         let exists = RFunction::from("exists")
    //             .param("x", method.clone())
    //             .call()?;
    //         RObject::to::<bool>(exists)
    //     })?;

    //     if !exists {
    //         // No such method; return an error
    //         let reply = FrontendMessage::RpcResultError(FrontendRpcError {
    //             id: id.to_string(),
    //             error: FrontendRpcErrorData {
    //                 code: JsonRpcErrorCode::MethodNotFound, // Method not found
    //                 message: format!("No such method: {}", request.method),
    //             },
    //         });
    //         let comm_msg = CommMsg::Rpc(id.to_string(), serde_json::to_value(reply)?);
    //         self.comm.outgoing_tx.send(comm_msg)?;
    //         return Ok(());
    //     }

    //     // Form an R function call from the request
    //     let result = r_task(|| {
    //         let mut call = RFunction::from(method);
    //         for param in request.params.iter() {
    //             let p = RObject::try_from(param.clone())?;
    //             call.add(p);
    //         }
    //         let result = call.call()?;
    //         Value::try_from(result)
    //     });

    //     // Convert the reply to a message we can send to the front end
    //     let reply = match result {
    //         Ok(value) => FrontendMessage::RpcResultResponse(FrontendRpcResult {
    //             id: id.to_string(),
    //             result: value,
    //         }),
    //         Err(err) => FrontendMessage::RpcResultError(FrontendRpcError {
    //             id: id.to_string(),
    //             error: FrontendRpcErrorData {
    //                 code: JsonRpcErrorCode::InternalError,
    //                 message: err.to_string(),
    //             },
    //         }),
    //     };

    //     let comm_msg = CommMsg::Rpc(id.to_string(), serde_json::to_value(reply)?);

    //     // Deliver the RPC reply to the front end over the comm channel
    //     if let Err(err) = self.comm.outgoing_tx.send(comm_msg) {
    //         log::error!("Error sending Positron event to front end: {}", err);
    //     };
    //     Ok(())
    // }
}
