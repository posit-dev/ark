//
// console_graphics.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use std::rc::Rc;

use amalthea::comm::plot_comm::IntrinsicSize;
use amalthea::comm::plot_comm::PlotRenderSettings;
use amalthea::wire::execute_request::CodeLocation;

use crate::console::Console;

impl Console {
    /// Push execution context to the graphics device when an execute request starts.
    ///
    /// Stores the execution_id, code, code_location, and optional sizing overrides
    /// so they can be captured when new plots are created during execution.
    pub(super) fn graphics_on_execute_request(
        &self,
        execution_id: String,
        code: String,
        code_location: Option<CodeLocation>,
        render_settings: Option<PlotRenderSettings>,
        intrinsic_size: Option<IntrinsicSize>,
    ) {
        self.device_context().set_execution_context(
            execution_id,
            code,
            code_location,
            render_settings,
            intrinsic_size,
        );
    }

    /// Process pending graphics changes before R waits for user input, for
    /// example from `readline()` or `menu()`, so the frontend shows the plot the
    /// user is being asked about. In Notebook mode this also runs before a
    /// `browser()` prompt that is routed to stdin.
    pub(super) fn graphics_on_input_request(&self) {
        Rc::clone(self.device_context()).process_changes_before_input(self);
    }

    /// Process pending graphics changes after an execute request completes.
    pub(super) fn graphics_on_did_execute_request(&self) {
        let dc = Rc::clone(self.device_context());
        dc.process_changes(self);
        dc.clear_execution_context();
    }
}
