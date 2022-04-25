/*
 * r_kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r_request::RRequest;
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
    console: Sender<Option<String>>,
    output: String,
}

impl RKernel {
    /// Create a new R kernel instance
    pub fn new(iopub: Sender<IOPubMessage>, console: Sender<Option<String>>) -> Self {
        Self {
            iopub: iopub,
            execution_count: 0,
            console: console,
            output: String::new(),
        }
    }

    /// Service an execution request from the front end
    pub fn fulfill_request(&mut self, req: RRequest) {
        match req {
            RRequest::ExecuteCode(req) => {
                self.handle_execute_request(&req);
            }
            RRequest::Shutdown(restart) => {
                self.console.send(None);
            }
        }
    }

    pub fn handle_execute_request(&mut self, req: &ExecuteRequest) {
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
        self.console.send(Some(req.code.clone())).unwrap();
    }

    /// Converts a data frame to HTML
    pub fn to_html(frame: &Robj) -> String {
        let names = frame.names().unwrap();
        let mut th = String::from("<tr>");
        for i in names {
            let h = format!("<th>{}</th>", i);
            th.push_str(h.as_str());
        }
        th.push_str("</tr>");
        let mut body = String::new();
        for i in 1..5 {
            body.push_str("<tr>");
            for j in 1..(frame.len() + 1) {
                trace!("formatting value at {}, {}", i, j);
                if let Ok(col) = frame.index(i) {
                    if let Ok(val) = col.index(j) {
                        if let Ok(s) = call!("toString", val) {
                            body.push_str(
                                format!("<td>{}</td>", String::from_robj(&s).unwrap()).as_str(),
                            )
                        }
                    }
                }
            }
            body.push_str("</tr>");
        }
        format!(
            "<table><thead>{}</thead><tbody>{}</tbody></table>",
            th, body
        )
    }

    /// Finishes the active execution request
    pub fn finish_request(&self) {
        let output = self.output.clone();

        // Look up computation result
        let mut data = serde_json::Map::new();
        data.insert("text/plain".to_string(), json!(output));
        trace!("Formatting value");
        let last = R!(.Last.value).unwrap();
        if last.is_frame() {
            data.insert("text/html".to_string(), json!(RKernel::to_html(&last)));
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
