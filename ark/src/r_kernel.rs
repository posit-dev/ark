/*
 * r_kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_result::ExecuteResult;
use log::{debug, trace, warn};
use serde_json::json;
use std::sync::mpsc::{Receiver, Sender};

pub struct RKernel {
    pub execution_count: u32,
    iopub: Sender<IOPubMessage>,
    console: Sender<String>,
    prompt: Receiver<String>,
    output: String,
}

impl RKernel {
    pub fn new(
        iopub: Sender<IOPubMessage>,
        console: Sender<String>,
        prompt: Receiver<String>,
    ) -> Self {
        Self {
            iopub: iopub,
            execution_count: 0,
            console: console,
            prompt: prompt,
            output: String::new(),
        }
    }

    pub fn execute_request(&mut self, req: ExecuteRequest) {
        self.output = String::new();

        // Increment counter if we are storing this execution in history
        if req.store_history {
            self.execution_count = self.execution_count + 1;
        }

        // If the code is not to be executed silently, re-broadcast the
        // execution to all frontends
        if !req.silent {
            if let Err(err) = self.iopub.send(IOPubMessage::ExecuteInput(ExecuteInput {
                code: req.code.clone(),
                execution_count: self.execution_count,
            })) {
                warn!(
                    "Could not broadcast execution input {} to all front ends: {}",
                    self.execution_count, err
                );
            }
        }

        self.console.send(req.code).unwrap();
        let prompt = self.prompt.recv().unwrap();
        trace!(
            "Completed execute request {} with R prompt: {} ... {}",
            self.execution_count,
            prompt,
            self.output
        );

        let data = json!({"text/plain": self.output });
        if let Err(err) = self.iopub.send(IOPubMessage::ExecuteResult(ExecuteResult {
            execution_count: self.execution_count,
            data: data,
            metadata: serde_json::Value::Null,
        })) {
            warn!(
                "Could not publish result of statement {} on iopub: {}",
                self.execution_count, err
            );
        }
    }

    pub fn write_console(&mut self, content: &str, otype: i32) {
        debug!("Write console {} from R: {}", otype, content);
        self.output.push_str(content);
    }
}
