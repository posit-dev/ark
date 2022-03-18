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
use extendr_api::prelude::*;
use log::{debug, trace, warn};
use serde_json::json;
use std::sync::mpsc::Sender;

/// Represents the Rust state of the R kernel
pub struct RKernel {
    pub execution_count: u32,
    iopub: Sender<IOPubMessage>,
    console: Sender<String>,
    output: String,
}

impl RKernel {
    /// Create a new R kernel instance
    pub fn new(iopub: Sender<IOPubMessage>, console: Sender<String>) -> Self {
        Self {
            iopub: iopub,
            execution_count: 0,
            console: console,
            output: String::new(),
        }
    }

    /// Service an execution request from the front end
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

        // Send the code to the R console to be evaluated
        self.console.send(req.code).unwrap();
    }

    /// Finishes the active execution request
    pub fn finish_request(&self) {
        let output = self.output.clone();

        // Look up computation result
        let mut data = serde_json::Map::new();
        data.insert("text/plain".to_string(), json!(output));
        let last = R!(.Last.value).unwrap();
        if last.is_frame() {
            let names = last.names().unwrap();
            let mut th = String::from("<tr>");
            for i in names {
                let h = format!("<th>{}</th>", i);
                th.push_str(h.as_str());
            }
            th.push_str("</tr>");
            data.insert(
                "text/html".to_string(),
                json!(format!(
                    "<table><caption>A data table: {}</caption>{}</table>",
                    last.len(),
                    th
                )),
            );
        }

        trace!("Sending kernel output: {}", self.output);
        if let Err(err) = self.iopub.send(IOPubMessage::ExecuteResult(ExecuteResult {
            execution_count: self.execution_count,
            data: serde_json::Value::Object(data),
            metadata: json!({}),
        })) {
            warn!(
                "Could not publish result of statement {} on iopub: {}",
                self.execution_count, err
            );
        }
    }

    /// Called from R when console data is written
    pub fn write_console(&mut self, content: &str, otype: i32) {
        debug!("Write console {} from R: {}", otype, content);
        // Accumulate output internally until R is finished executing
        self.output.push_str(content);
    }
}
