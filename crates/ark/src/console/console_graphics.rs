//
// console_graphics.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use std::rc::Rc;

use amalthea::wire::execute_request::CodeLocation;
use amalthea::wire::execute_request::ExecuteRequestPositron;

use crate::console::Console;

impl Console {
    /// Pushes execution context to the graphics device when an execute request starts.
    ///
    /// The context attributes newly created plots to the execution and captures its
    /// requested figure dimensions and device pixel ratio.
    pub(super) fn graphics_on_execute_request(
        &self,
        execution_id: String,
        code: String,
        code_location: Option<CodeLocation>,
        positron: Option<&ExecuteRequestPositron>,
    ) {
        self.device_context()
            .set_execution_context(execution_id, code, code_location, positron);
    }

    /// Process pending graphics changes after an execute request completes.
    pub(super) fn graphics_on_did_execute_request(&self) {
        let dc = Rc::clone(self.device_context());
        dc.process_changes(self);
        dc.clear_execution_context();
        dc.clear_pending_origin();
    }
}
