//
// console_graphics.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use amalthea::comm::plot_comm::IntrinsicSize;
use amalthea::comm::plot_comm::PlotRenderSettings;
use amalthea::wire::execute_request::CodeLocation;

use crate::console::Console;
use crate::plots::graphics_device::PLOT_COMM_NAME;

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

    /// Process pending graphics changes after an execute request completes.
    ///
    /// FIXME: Takes `&mut self` because registering a new plot comm mutates the
    /// comms map.
    pub(super) fn graphics_on_did_execute_request(&mut self) {
        let pending = {
            let dc = self.device_context();
            let pending = dc.process_changes(self);
            dc.clear_execution_context();
            dc.clear_pending_origin();
            pending
        };

        // FIXME: Should move to the device context once `comm_open_backend()`
        // no longer requires `&mut Console`.
        if let Some(pending) = pending {
            match self.comm_open_backend(PLOT_COMM_NAME, pending.handler) {
                Ok(comm_id) => self.device_context().register_comm(pending.id, comm_id),
                Err(err) => log::error!("Failed to register plot comm: {err:?}"),
            }
        }
    }
}
