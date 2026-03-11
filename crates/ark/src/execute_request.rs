//
// execute_request.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;

use crate::console::Console;

/// Returns the currently active execute request as an R named list,
/// or NULL if no execute request is in flight.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_active_request() -> anyhow::Result<SEXP> {
    let Some(req) = Console::get().get_active_execute_request() else {
        return Ok(R_NilValue);
    };

    let json = serde_json::to_value(req)?;
    let r_obj = RObject::try_from(json)?;
    Ok(r_obj.sexp)
}
