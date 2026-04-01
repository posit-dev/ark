//
// ui_comm.rs
//
// Copyright (C) 2023-2026 by Posit Software, PBC
//
//

use std::path::PathBuf;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::ui_comm::CallMethodParams;
use amalthea::comm::ui_comm::DidChangePlotsRenderSettingsParams;
use amalthea::comm::ui_comm::EvalResult;
use amalthea::comm::ui_comm::EvaluateCodeParams;
use amalthea::comm::ui_comm::FrontendReadyParams;
use amalthea::comm::ui_comm::PromptStateParams;
use amalthea::comm::ui_comm::UiBackendReply;
use amalthea::comm::ui_comm::UiBackendRequest;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::comm::ui_comm::WorkingDirectoryParams;
use harp::eval::parse_eval_global;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use serde_json::Value;
use stdext::result::ResultExt;
use tokio::sync::mpsc::UnboundedSender as AsyncUnboundedSender;

use crate::comm_handler::handle_rpc_request;
use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::comm_handler::EnvironmentChanged;
use crate::console::Console;
use crate::console::ConsoleOutputCapture;
use crate::modules::ARK_ENVS;
use crate::plots::graphics_device::GraphicsDeviceNotification;

pub const UI_COMM_NAME: &str = "positron.ui";

/// Data sent by the frontend in the `comm_open` message for the UI comm.
#[derive(Debug, serde::Deserialize)]
struct UiCommOpenData {
    #[serde(default)]
    console_width: Option<i32>,
}

/// Comm handler for the Positron UI comm.
#[derive(Debug)]
pub struct UiComm {
    graphics_device_tx: AsyncUnboundedSender<GraphicsDeviceNotification>,
    working_directory: PathBuf,
    comm_open_data: UiCommOpenData,
}

impl CommHandler for UiComm {
    fn handle_open(&mut self, ctx: &CommHandlerContext) {
        // Set initial console width from the comm_open data, if provided.
        if let Some(width) = self.comm_open_data.console_width {
            if let Err(err) = RFunction::from(".ps.rpc.setConsoleWidth")
                .param("width", RObject::from(width))
                .call()
            {
                log::warn!("Failed to set initial console width: {err:?}");
            }
        }

        // At open time there's no EnvironmentChanged event carrying prompts,
        // so read the R options directly. This is fine for the initial state —
        // browser/debug prompts will arrive via `handle_environment()` later.
        let input_prompt = harp::get_input_prompt();
        let continuation_prompt = harp::get_continuation_prompt();
        self.refresh(&input_prompt, &continuation_prompt, ctx);
    }

    fn handle_msg(&mut self, msg: CommMsg, ctx: &CommHandlerContext) {
        handle_rpc_request(&ctx.outgoing_tx, UI_COMM_NAME, msg, |req| {
            self.handle_rpc(req)
        });
    }

    fn handle_environment(&mut self, event: &EnvironmentChanged, ctx: &CommHandlerContext) {
        let EnvironmentChanged::Execution {
            input_prompt,
            continuation_prompt,
        } = event
        else {
            return;
        };
        self.refresh(input_prompt, continuation_prompt, ctx);
    }
}

impl UiComm {
    pub(crate) fn new(
        graphics_device_tx: AsyncUnboundedSender<GraphicsDeviceNotification>,
        comm_open_data: Value,
    ) -> Self {
        let comm_open_data: UiCommOpenData =
            serde_json::from_value(comm_open_data).unwrap_or_else(|err| {
                log::warn!("Failed to deserialize UI comm_open data: {err:?}");
                UiCommOpenData {
                    console_width: None,
                }
            });

        Self {
            graphics_device_tx,
            working_directory: PathBuf::new(),
            comm_open_data,
        }
    }

    fn handle_rpc(&mut self, request: UiBackendRequest) -> anyhow::Result<UiBackendReply> {
        match request {
            UiBackendRequest::CallMethod(params) => self.handle_call_method(params),
            UiBackendRequest::DidChangePlotsRenderSettings(params) => {
                self.handle_did_change_plot_render_settings(params)
            },
            UiBackendRequest::FrontendReady(params) => self.handle_frontend_ready(params),
            UiBackendRequest::EvaluateCode(params) => self.handle_evaluate_code(params),
        }
    }

    fn handle_call_method(&self, request: CallMethodParams) -> anyhow::Result<UiBackendReply> {
        log::trace!("Handling '{}' frontend RPC method", request.method);

        // Today, all RPCs are fulfilled by R directly. Check to see if an R
        // method of the appropriate name is defined.
        //
        // Consider: In the future, we may want to allow requests to be
        // fulfilled here on the Rust side, with only some requests forwarded to
        // R; Rust methods may wish to establish their own RPC handlers.

        let method = format!(".ps.rpc.{}", request.method);

        let exists_obj = RFunction::from("exists")
            .param("x", method.clone())
            .call()?;
        let exists: bool = exists_obj.try_into()?;

        if !exists {
            let method = &request.method;
            return Err(anyhow::anyhow!("No such method: {method}"));
        }

        let mut call = RFunction::from(method);
        for param in request.params.iter() {
            let p = RObject::try_from(param.clone())?;
            call.add(p);
        }
        let result = call.call()?;
        let result = Value::try_from(result)?;

        Ok(UiBackendReply::CallMethodReply(result))
    }

    fn handle_did_change_plot_render_settings(
        &self,
        params: DidChangePlotsRenderSettingsParams,
    ) -> anyhow::Result<UiBackendReply> {
        // The frontend shouldn't send invalid sizes but be defensive. Sometimes
        // the plot container is in a strange state when it's hidden.
        if params.settings.size.height <= 0 || params.settings.size.width <= 0 {
            return Err(anyhow::anyhow!(
                "Got invalid plot render size: {size:?}",
                size = params.settings.size,
            ));
        }

        self.graphics_device_tx
            .send(GraphicsDeviceNotification::DidChangePlotRenderSettings(
                params.settings,
            ))
            .map_err(|err| anyhow::anyhow!("Failed to send plot render settings: {err}"))?;

        Ok(UiBackendReply::DidChangePlotsRenderSettingsReply())
    }

    fn handle_frontend_ready(&self, params: FrontendReadyParams) -> anyhow::Result<UiBackendReply> {
        log::info!("Frontend ready (start_type = {})", params.start_type);

        if params.start_type == "reconnect" {
            RFunction::from(".ps.run_session_reconnect_hooks")
                .call_in(ARK_ENVS.positron_ns)
                .warn_on_err();
        } else {
            RFunction::from(".ps.run_session_init_hooks")
                .param("start_type", RObject::from(params.start_type.as_str()))
                .call_in(ARK_ENVS.positron_ns)
                .warn_on_err();
        }

        Ok(UiBackendReply::FrontendReadyReply())
    }

    fn handle_evaluate_code(&self, params: EvaluateCodeParams) -> anyhow::Result<UiBackendReply> {
        log::trace!("Evaluating code: {}", params.code);

        let mut capture = if Console::is_initialized() {
            Console::get_mut().start_capture()
        } else {
            ConsoleOutputCapture::dummy()
        };

        let value = parse_eval_global(&params.code);

        let output = capture.take();
        drop(capture);

        match value {
            Ok(evaluated) => {
                let result = Value::try_from(evaluated)?;
                Ok(UiBackendReply::EvaluateCodeReply(EvalResult {
                    result,
                    output,
                }))
            },
            Err(err) => {
                let message = match err {
                    harp::Error::TryCatchError(err) => err.message,
                    harp::Error::ParseError { message, .. } => message,
                    harp::Error::ParseSyntaxError { message } => message,
                    _ => format!("{err}"),
                };
                Err(anyhow::anyhow!("{message}"))
            },
        }
    }

    fn refresh(&mut self, input_prompt: &str, continuation_prompt: &str, ctx: &CommHandlerContext) {
        ctx.send_event(&UiFrontendEvent::PromptState(PromptStateParams {
            input_prompt: input_prompt.to_string(),
            continuation_prompt: continuation_prompt.to_string(),
        }));
        self.refresh_working_directory(ctx).log_err();
    }

    /// Checks for changes to the working directory, and sends an event to the
    /// frontend if the working directory has changed.
    fn refresh_working_directory(&mut self, ctx: &CommHandlerContext) -> anyhow::Result<()> {
        let mut new_working_directory = std::env::current_dir()?;

        if new_working_directory != self.working_directory {
            self.working_directory = new_working_directory.clone();

            // Attempt to alias the directory, if it's within the home directory
            if let Some(home_dir) = home::home_dir() {
                if let Ok(stripped_dir) = new_working_directory.strip_prefix(home_dir) {
                    let mut new_path = PathBuf::from("~");
                    new_path.push(stripped_dir);
                    new_working_directory = new_path;
                }
            }

            ctx.send_event(&UiFrontendEvent::WorkingDirectory(WorkingDirectoryParams {
                directory: new_working_directory.to_string_lossy().to_string(),
            }));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use amalthea::comm::base_comm::JsonRpcError;
    use amalthea::comm::comm_channel::CommMsg;
    use amalthea::comm::event::CommEvent;
    use amalthea::comm::ui_comm::CallMethodParams;
    use amalthea::comm::ui_comm::EvalResult;
    use amalthea::comm::ui_comm::EvaluateCodeParams;
    use amalthea::comm::ui_comm::UiBackendReply;
    use amalthea::comm::ui_comm::UiBackendRequest;
    use amalthea::socket::comm::CommOutgoingTx;
    use amalthea::socket::iopub::IOPubMessage;
    use ark_test::dummy_jupyter_header;
    use ark_test::IOPubReceiverExt;
    use crossbeam::channel::bounded;
    use serde_json::Value;

    use super::*;
    use crate::comm_handler::CommHandlerContext;
    use crate::r_task;

    fn setup_ui_comm(
        iopub_tx: crossbeam::channel::Sender<IOPubMessage>,
    ) -> (UiComm, CommHandlerContext) {
        let comm_id = uuid::Uuid::new_v4().to_string();

        let outgoing_tx = CommOutgoingTx::new(comm_id, iopub_tx);
        let (comm_event_tx, _) = bounded::<CommEvent>(10);
        let ctx = CommHandlerContext::new(outgoing_tx, comm_event_tx);

        let (graphics_device_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let handler = UiComm::new(graphics_device_tx, serde_json::Value::Null);

        (handler, ctx)
    }

    #[test]
    fn test_ui_comm() {
        let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

        let old_width = r_task(move || {
            let (mut handler, ctx) = setup_ui_comm(iopub_tx);

            // Get the current console width
            let old_width: i32 = harp::get_option("width").try_into().unwrap();

            // Send a setConsoleWidth RPC
            let msg = CommMsg::Rpc {
                id: String::from("test-id-1"),
                parent_header: dummy_jupyter_header(),
                data: serde_json::to_value(UiBackendRequest::CallMethod(CallMethodParams {
                    method: String::from("setConsoleWidth"),
                    params: vec![Value::from(123)],
                }))
                .unwrap(),
            };
            handler.handle_msg(msg, &ctx);

            // Assert that the console width changed
            let new_width: i32 = harp::get_option("width").try_into().unwrap();
            assert_eq!(new_width, 123);

            // Now try to invoke an RPC that doesn't exist
            let msg = CommMsg::Rpc {
                id: String::from("test-id-2"),
                parent_header: dummy_jupyter_header(),
                data: serde_json::to_value(UiBackendRequest::CallMethod(CallMethodParams {
                    method: String::from("thisRpcDoesNotExist"),
                    params: vec![],
                }))
                .unwrap(),
            };
            handler.handle_msg(msg, &ctx);

            old_width
        });

        // Check first response (setConsoleWidth)
        let response = iopub_rx.recv_comm_msg();
        match response {
            CommMsg::Rpc { id, data, .. } => {
                let result = serde_json::from_value::<UiBackendReply>(data).unwrap();
                assert_eq!(id, "test-id-1");
                assert_eq!(
                    result,
                    UiBackendReply::CallMethodReply(Value::from(old_width))
                );
            },
            _ => panic!("Unexpected response: {:?}", response),
        }

        // Check second response (non-existent method error)
        let response = iopub_rx.recv_comm_msg();
        match response {
            CommMsg::Rpc { id, data, .. } => {
                let _reply = serde_json::from_value::<JsonRpcError>(data).unwrap();
                assert_eq!(id, "test-id-2");
            },
            _ => panic!("Unexpected response: {:?}", response),
        }
    }

    #[test]
    fn test_evaluate_code() {
        let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

        r_task(move || {
            let (mut handler, ctx) = setup_ui_comm(iopub_tx);

            // Pure result with no output (e.g. 1 + 1)
            let msg = CommMsg::Rpc {
                id: String::from("eval-1"),
                parent_header: dummy_jupyter_header(),
                data: serde_json::to_value(UiBackendRequest::EvaluateCode(EvaluateCodeParams {
                    code: String::from("1 + 1"),
                }))
                .unwrap(),
            };
            handler.handle_msg(msg, &ctx);
        });

        let response = iopub_rx.recv_comm_msg();
        match response {
            CommMsg::Rpc { data, .. } => {
                let result = serde_json::from_value::<UiBackendReply>(data).unwrap();
                assert_eq!(
                    result,
                    UiBackendReply::EvaluateCodeReply(EvalResult {
                        result: Value::from(2.0),
                        // Output capture relies on Console::start_capture(), which is
                        // not available in unit tests (Console is not initialized).
                        // Output capture is exercised in integration tests instead.
                        output: String::from(""),
                    })
                );
            },
            _ => panic!("Unexpected response: {:?}", response),
        }
    }
}
