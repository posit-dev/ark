//
// frontend.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::comm::frontend_comm::FrontendMessage;
use amalthea::events::PositronEvent;
use amalthea::socket::comm::CommSocket;
use amalthea::wire::client_event::ClientEvent;
use crossbeam::channel::Sender;
use stdext::spawn;

/// PositronFrontend is a wrapper around a comm channel whose lifetime matches
/// that of the Positron front end. It is used to perform communication with the
/// front end that isn't scoped to any particular view.
pub struct PositronFrontend {
    pub event_tx: Sender<PositronEvent>,
}

impl PositronFrontend {
    pub fn new(comm: CommSocket) -> Self {
        // Create a sender-receiver pair for Positron global events
        let (event_tx, event_rx) = crossbeam::channel::unbounded::<PositronEvent>();

        // Create a copy of the comm channel into which we will feed events from
        // the backend
        let comm_tx = comm.outgoing_tx.clone();

        // Wait for events from the backend and forward them over the channel
        spawn!("ark-comm-frontend", move || loop {
            // Read the event from the backend
            let event = match event_rx.recv() {
                Ok(event) => event,
                Err(err) => {
                    log::error!(
                        "Error receiving Positron event; closing event listener: {}",
                        err
                    );
                    // Most likely the channel was closed, so we should stop the thread
                    break;
                },
            };

            // Convert the event to a client event that the frontend can understand
            let comm_evt = ClientEvent::try_from(event.clone()).unwrap();

            // Convert the client event to a message we can send to the front end
            let frontend_evt = FrontendMessage::Event(comm_evt);
            let comm_msg = CommChannelMsg::Data(serde_json::to_value(frontend_evt).unwrap());

            // Deliver the event to the front end over the comm channel
            if let Err(err) = comm_tx.send(comm_msg) {
                log::error!("Error sending Positron event to front end: {}", err);
            };
        });
        Self { event_tx }
    }
}
