//
// frontend.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::comm::frontend_comm::FrontendMessage;
use amalthea::comm::frontend_comm::FrontendRpcRequest;
use amalthea::events::PositronEvent;
use amalthea::socket::comm::CommSocket;
use amalthea::wire::client_event::ClientEvent;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use log::info;
use stdext::spawn;

use crate::r_task;

/// PositronFrontend is a wrapper around a comm channel whose lifetime matches
/// that of the Positron front end. It is used to perform communication with the
/// front end that isn't scoped to any particular view.
pub struct PositronFrontend {
    comm: CommSocket,
    event_rx: Receiver<PositronEvent>,
}

impl PositronFrontend {
    pub fn start(comm: CommSocket) -> Sender<PositronEvent> {
        // Create a sender-receiver pair for Positron global events
        let (event_tx, event_rx) = crossbeam::channel::unbounded::<PositronEvent>();

        spawn!("ark-comm-frontend", move || loop {
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
                                "Error receiving Positron event; closing event listener: {}",
                                err
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

    fn dispatch_event(&self, event: &PositronEvent) {
        // Convert the event to a client event that the frontend can understand
        let comm_evt = ClientEvent::try_from(event.clone()).unwrap();

        // Convert the client event to a message we can send to the front end
        let frontend_evt = FrontendMessage::Event(comm_evt);
        let comm_msg = CommChannelMsg::Data(serde_json::to_value(frontend_evt).unwrap());

        // Deliver the event to the front end over the comm channel
        if let Err(err) = self.comm.outgoing_tx.send(comm_msg) {
            log::error!("Error sending Positron event to front end: {}", err);
        };
    }

    /**
     * Handles a comm message from the front end.
     *
     * Returns true if the thread should continue, false if it should exit.
     */
    fn handle_comm_message(&self, msg: &CommChannelMsg) -> bool {
        match msg {
            CommChannelMsg::Data(data) => {
                // We don't really expect to receive data messages from the
                // front end; they are events
                log::warn!("Unexpected data message from front end: {:?}", data);
                true
            },
            CommChannelMsg::Close => {
                // The front end has closed the connection; let the
                // thread exit.
                false
            },
            CommChannelMsg::Rpc(id, request) => {
                self.handle_rpc_request(id, request);
                true
            },
        }
    }

    /**
     * Handles an RPC request from the front end.
     */
    fn handle_rpc_request(&self, id: &str, request: &serde_json::Value) {
        // Parse the request from the front end
        let request = match serde_json::from_value::<FrontendRpcRequest>(request.clone()) {
            Ok(request) => request,
            Err(err) => {
                log::error!("Invalid RPC request from front end: {:?}", err);
                return;
            },
        };
        let result = r_task(|| unsafe {
            let call = RFunction::from(format!(".ps.rpc.{}", request.method));
            for param in request.params.iter() {
                // call.add(param);
            }
        });
        log::info!("RPC request from front end: {:?}", request);
    }
}
