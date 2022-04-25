/*
 * r_request.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::wire::execute_request::ExecuteRequest;

/// Represents requests to the primary R execution thread.
pub enum RRequest {
    /// Fulfill an execution request from the front end
    ExecuteCode(ExecuteRequest),

    /// Shut down the R execution thread
    Shutdown(bool),
}
