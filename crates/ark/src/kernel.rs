//
// kernel.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::result::Result::Err;
use std::result::Result::Ok;
use std::sync::atomic::AtomicBool;

use amalthea::events::PositronEvent;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::input_request::ShellInputRequest;
use amalthea::wire::stream::Stream;
use amalthea::wire::stream::StreamOutput;
use anyhow::*;
use bus::Bus;
use crossbeam::atomic::AtomicCell;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_lock;
use harp::r_symbol;
use libR_sys::*;
use log::*;
use stdext::unwrap;

use crate::interface::ConsoleInput;
use crate::request::KernelRequest;

/// Represents whether an error occurred during R code execution.
pub static R_ERROR_OCCURRED: AtomicBool = AtomicBool::new(false);
pub static R_ERROR_EVALUE: AtomicCell<String> = AtomicCell::new(String::new());
pub static R_ERROR_TRACEBACK: AtomicCell<Vec<String>> = AtomicCell::new(Vec::new());

/// Represents the Rust state of the R kernel
pub struct Kernel {
    /** Counter used to populate `In[n]` and `Out[n]` prompts */
    pub execution_count: u32,
    pub input_request_tx: Option<Sender<ShellInputRequest>>,

    iopub_tx: Sender<IOPubMessage>,
    kernel_init_tx: Bus<KernelInfo>,
    event_tx: Option<Sender<PositronEvent>>,
    banner: String,
    stdout: String,
    stderr: String,
    initializing: bool,
}

/// Represents kernel metadata (available after the kernel has fully started)
#[derive(Debug, Clone)]
pub struct KernelInfo {
    pub version: String,
    pub banner: String,
}

impl Kernel {
    /// Create a new R kernel instance
    pub fn new(iopub_tx: Sender<IOPubMessage>, kernel_init_tx: Bus<KernelInfo>) -> Self {
        Self {
            execution_count: 0,
            iopub_tx,
            kernel_init_tx,
            input_request_tx: None,
            event_tx: None,
            banner: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            initializing: true,
        }
    }

    /// Completes the kernel's initialization
    pub fn complete_initialization(&mut self) {
        if self.initializing {
            let version = r_lock! {
                let version = Rf_findVarInFrame(R_BaseNamespace, r_symbol!("R.version.string"));
                RObject::new(version).to::<String>().unwrap()
            };

            let kernel_info = KernelInfo {
                version: version.clone(),
                banner: self.banner.clone(),
            };

            debug!("Sending kernel info: {}", version);
            self.kernel_init_tx.broadcast(kernel_info);
            self.initializing = false;
        } else {
            warn!("Initialization already complete!");
        }
    }

    /// Service an execution request from the front end
    pub fn fulfill_request(&mut self, req: &KernelRequest) {
        match req {
            KernelRequest::EstablishInputChannel(sender) => {
                self.establish_input_handler(sender.clone())
            },
            KernelRequest::EstablishEventChannel(sender) => {
                self.establish_event_handler(sender.clone())
            },
            KernelRequest::DeliverEvent(event) => self.handle_event(event),
        }
    }

    /// Handle an event from the back end to the front end
    pub fn handle_event(&mut self, event: &PositronEvent) {
        if let Err(err) = self.iopub_tx.send(IOPubMessage::Event(event.clone())) {
            warn!("Error attempting to deliver client event: {}", err);
        }
    }

    /// Handle an execute request from the front end
    pub fn handle_execute_request(&mut self, req: &ExecuteRequest) -> (ConsoleInput, u32) {
        // Clear error occurred flag
        R_ERROR_OCCURRED.store(false, std::sync::atomic::Ordering::Release);

        // Initialize stdout, stderr
        self.stdout = String::new();
        self.stderr = String::new();

        // Increment counter if we are storing this execution in history
        if req.store_history {
            self.execution_count = self.execution_count + 1;
        }

        // If the code is not to be executed silently, re-broadcast the
        // execution to all frontends
        if !req.silent {
            if let Err(err) = self.iopub_tx.send(IOPubMessage::ExecuteInput(ExecuteInput {
                code: req.code.clone(),
                execution_count: self.execution_count,
            })) {
                warn!(
                    "Could not broadcast execution input {} to all front ends: {}",
                    self.execution_count, err
                );
            }
        }

        // Return the code to the R console to be evaluated and the corresponding exec count
        (ConsoleInput::Input(req.code.clone()), self.execution_count)
    }

    /// Converts a data frame to HTML
    pub fn to_html(frame: SEXP) -> Result<String> {
        unsafe {
            let result = RFunction::from(".ps.format.toHtml")
                .add(frame)
                .call()?
                .to::<String>()?;
            Ok(result)
        }
    }

    /// Called from R when console data is written.
    pub fn write_console(&mut self, content: &str, stream: Stream) {
        if self.initializing {
            // During init, consider all output to be part of the startup banner
            self.banner.push_str(content);
            return;
        }

        let buffer = match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        };

        // Append content to buffer.
        buffer.push_str(content);

        // Stream output via the IOPub channel.
        let message = IOPubMessage::Stream(StreamOutput {
            name: stream,
            text: content.to_string(),
        });

        unwrap!(self.iopub_tx.send(message), Err(error) => {
            log::error!("{}", error);
        });
    }

    /// Establishes the input handler for the kernel to request input from the
    /// user
    pub fn establish_input_handler(&mut self, input_request_tx: Sender<ShellInputRequest>) {
        self.input_request_tx = Some(input_request_tx);
    }

    /// Establishes the event handler for the kernel to send events to the
    /// Positron front end. This event handler is used to send global events
    /// that are not scoped to any particular view. The `Sender` here is a
    /// channel that is connected to a `positron.frontEnd` comm.
    pub fn establish_event_handler(&mut self, event_tx: Sender<PositronEvent>) {
        self.event_tx = Some(event_tx);
    }

    /// Sends an event to the front end (Positron-specific)
    pub fn send_event(&self, event: PositronEvent) {
        info!("Sending Positron event: {:?}", event);
        if let Some(event_tx) = &self.event_tx {
            if let Err(err) = event_tx.send(event) {
                warn!("Error sending event to front end: {}", err);
            }
        } else {
            warn!(
                "Discarding event {:?}; no Positron front end connected",
                event
            );
        }
    }
}
