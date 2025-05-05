//
// ui.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::ui_comm::CallMethodParams;
use amalthea::comm::ui_comm::DidChangePlotsRenderSettingsParams;
use amalthea::comm::ui_comm::UiBackendReply;
use amalthea::comm::ui_comm::UiBackendRequest;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::stdin::StdInRequest;
use amalthea::wire::input_request::UiCommFrontendRequest;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use serde_json::Value;
use stdext::spawn;
use stdext::unwrap;

use crate::plots::graphics_device::GraphicsDeviceNotification;
use crate::r_task;

#[derive(Debug)]
pub enum UiCommMessage {
    Event(UiFrontendEvent),
    Request(UiCommFrontendRequest),
}

/// UiComm is a wrapper around a comm channel whose lifetime matches
/// that of the Positron UI frontend. It is used to perform communication with the
/// frontend that isn't scoped to any particular view.
pub struct UiComm {
    comm: CommSocket,
    ui_comm_rx: Receiver<UiCommMessage>,
    stdin_request_tx: Sender<StdInRequest>,
    graphics_device_tx: Sender<GraphicsDeviceNotification>,
}

impl UiComm {
    pub(crate) fn start(
        comm: CommSocket,
        stdin_request_tx: Sender<StdInRequest>,
        graphics_device_tx: Sender<GraphicsDeviceNotification>,
    ) -> Sender<UiCommMessage> {
        // Create a sender-receiver pair for Positron global events
        let (ui_comm_tx, ui_comm_rx) = crossbeam::channel::unbounded::<UiCommMessage>();

        spawn!("ark-comm-ui", move || {
            let frontend = Self {
                comm,
                ui_comm_rx,
                stdin_request_tx,
                graphics_device_tx,
            };
            frontend.execution_thread();
        });

        ui_comm_tx
    }

    fn execution_thread(&self) {
        loop {
            // Wait for an event on either the event channel (which forwards
            // Positron events to the frontend) or the comm channel (which
            // receives requests from the frontend)
            select! {
                recv(&self.ui_comm_rx) -> msg => {
                    let msg = unwrap!(msg, Err(err) => {
                        log::error!(
                            "Error receiving Positron event; closing event listener: {err:?}"
                        );
                        // Most likely the channel was closed, so we should stop the thread
                        break;
                    });
                    match msg {
                        UiCommMessage::Event(event) => self.dispatch_event(&event),
                        UiCommMessage::Request(request) => self.call_frontend_method(request).unwrap(),
                    }
                },

                recv(&self.comm.incoming_rx) -> msg => {
                    match msg {
                        Ok(msg) => {
                            if !self.handle_comm_message(msg) {
                                log::info!("UI comm {} closing by request from frontend.", self.comm.comm_id);
                                break;
                            }
                        },
                        Err(err) => {
                            log::error!("Error receiving message from frontend: {:?}", err);
                            break;
                        },
                    }
                },
            }
        }
    }

    fn dispatch_event(&self, event: &UiFrontendEvent) {
        let json = serde_json::to_value(event).unwrap();

        // Deliver the event to the frontend over the comm channel
        if let Err(err) = self.comm.outgoing_tx.send(CommMsg::Data(json)) {
            log::error!("Error sending UI event to frontend: {}", err);
        };
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
            .handle_request(message.clone(), |req| self.handle_backend_method(req))
        {
            return true;
        }

        // We don't really expect to receive data messages from the
        // frontend; they are events
        log::warn!("Unexpected data message from frontend: {message:?}");
        true
    }

    /**
     * Handles an RPC request from the frontend.
     */
    fn handle_backend_method(
        &self,
        request: UiBackendRequest,
    ) -> anyhow::Result<UiBackendReply, anyhow::Error> {
        match request {
            UiBackendRequest::CallMethod(request) => self.handle_call_method(request),
            UiBackendRequest::DidChangePlotsRenderSettings(params) => {
                self.handle_did_change_plot_render_settings(params)
            },
        }
    }

    fn handle_call_method(
        &self,
        request: CallMethodParams,
    ) -> anyhow::Result<UiBackendReply, anyhow::Error> {
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

    fn handle_did_change_plot_render_settings(
        &self,
        params: DidChangePlotsRenderSettingsParams,
    ) -> anyhow::Result<UiBackendReply, anyhow::Error> {
        self.graphics_device_tx
            .send(GraphicsDeviceNotification::DidChangePlotRenderSettings(
                params.settings,
            ))
            .unwrap();

        Ok(UiBackendReply::DidChangePlotsRenderSettingsReply())
    }

    /**
     * Send an RPC request to the frontend.
     */
    fn call_frontend_method(&self, request: UiCommFrontendRequest) -> anyhow::Result<()> {
        let comm_msg = StdInRequest::Comm(request);
        self.stdin_request_tx.send(comm_msg)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use amalthea::comm::base_comm::JsonRpcError;
    use amalthea::comm::comm_channel::CommMsg;
    use amalthea::comm::ui_comm::BusyParams;
    use amalthea::comm::ui_comm::CallMethodParams;
    use amalthea::comm::ui_comm::UiBackendReply;
    use amalthea::comm::ui_comm::UiBackendRequest;
    use amalthea::comm::ui_comm::UiFrontendEvent;
    use amalthea::socket::comm::CommInitiator;
    use amalthea::socket::comm::CommSocket;
    use amalthea::socket::stdin::StdInRequest;
    use crossbeam::channel::bounded;
    use harp::exec::RFunction;
    use harp::exec::RFunctionExt;
    use harp::object::RObject;
    use serde_json::Value;

    use crate::plots::graphics_device::GraphicsDeviceNotification;
    use crate::r_task::r_task;
    use crate::ui::UiComm;
    use crate::ui::UiCommMessage;

    #[test]
    fn test_ui_comm() {
        // Create a sender/receiver pair for the comm channel.
        let comm_socket = CommSocket::new(
            CommInitiator::FrontEnd,
            String::from("test-ui-comm-id"),
            String::from("positron.UI"),
        );

        // Communication channel between the main thread and the Amalthea
        // StdIn socket thread
        let (stdin_request_tx, _stdin_request_rx) = bounded::<StdInRequest>(1);

        let (graphics_device_tx, _graphics_device_rx) =
            crossbeam::channel::unbounded::<GraphicsDeviceNotification>();

        // Create a frontend instance, get access to the sender channel
        let ui_comm_tx = UiComm::start(comm_socket.clone(), stdin_request_tx, graphics_device_tx);

        // Get the current console width
        let old_width = r_task(|| unsafe {
            let width = RFunction::from("getOption")
                .param("x", "width")
                .call()
                .unwrap();
            RObject::to::<i32>(width).unwrap()
        });

        // Send a message to the frontend
        let id = String::from("test-id-1");
        let request = UiBackendRequest::CallMethod(CallMethodParams {
            method: String::from("setConsoleWidth"),
            params: vec![Value::from(123)],
        });
        comm_socket
            .incoming_tx
            .send(CommMsg::Rpc(id, serde_json::to_value(request).unwrap()))
            .unwrap();

        // Wait for the reply; this should be a FrontendRpcResult. We don't wait
        // more than a second since this should be quite fast and we don't want to
        // hang the test suite if it doesn't return.
        let response = comm_socket
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let result = serde_json::from_value::<UiBackendReply>(result).unwrap();
                assert_eq!(id, "test-id-1");
                // This RPC should return the old width
                assert_eq!(
                    result,
                    UiBackendReply::CallMethodReply(Value::from(old_width))
                );
            },
            _ => panic!("Unexpected response: {:?}", response),
        }

        // Get the new console width
        let new_width = r_task(|| unsafe {
            let width = RFunction::from("getOption")
                .param("x", "width")
                .call()
                .unwrap();
            RObject::to::<i32>(width).unwrap()
        });

        // Assert that the console width changed
        assert_eq!(new_width, 123);

        // Now try to invoke an RPC that doesn't exist
        let id = String::from("test-id-2");
        let request = UiBackendRequest::CallMethod(CallMethodParams {
            method: String::from("thisRpcDoesNotExist"),
            params: vec![],
        });
        comm_socket
            .incoming_tx
            .send(CommMsg::Rpc(id, serde_json::to_value(request).unwrap()))
            .unwrap();

        // Wait for the reply
        let response = comm_socket
            .outgoing_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match response {
            CommMsg::Rpc(id, result) => {
                println!("Got RPC result: {:?}", result);
                let _reply = serde_json::from_value::<JsonRpcError>(result).unwrap();
                // Ensure that the error code is -32601 (method not found)
                assert_eq!(id, "test-id-2");

                // TODO: This should normally throw a `MethodNotFound` but
                // that's currently a bit hard because of the nested method
                // call. One way to solve this would be for RPC handler
                // functions to return a typed JSON-RPC error instead of a
                // `anyhow::Result`. Then we could return a `MethodNotFound` from
                // `callMethod()`.
                //
                // assert_eq!(reply.error.code, JsonRpcErrorCode::MethodNotFound);
            },
            _ => panic!("Unexpected response: {:?}", response),
        }

        // Mark not busy (this prevents the frontend comm from being closed due to
        // the Sender being dropped)
        ui_comm_tx
            .send(UiCommMessage::Event(UiFrontendEvent::Busy(BusyParams {
                busy: false,
            })))
            .unwrap();
    }
}
