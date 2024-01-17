//
// util.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::os::raw::c_char;

use harp::object::RObject;
use libR_shim::*;
use libr::R_NilValue;
use libr::Rf_mkString;

/// Shows a message in the Positron frontend
#[harp::register]
pub unsafe extern "C" fn ps_log_error(message: SEXP) -> anyhow::Result<SEXP> {
    let message = RObject::view(message).to::<String>();
    if let Ok(message) = message {
        log::error!("{}", message);
    }

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_object_id(object: SEXP) -> anyhow::Result<SEXP> {
    let value = format!("{:p}", object);
    return Ok(Rf_mkString(value.as_ptr() as *const c_char));
}
