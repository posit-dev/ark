//
// viewer.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CString;

use harp::object::RObject;
use libR_sys::Rf_error;
use libR_sys::SEXP;

#[harp::register]
pub unsafe extern "C" fn ps_to_json(obj: SEXP) -> SEXP {
    let obj = RObject::view(obj);
    match serde_json::Value::try_from(obj) {
        Ok(value) => {
            // Serialize the value to a string
            let json = serde_json::to_string_pretty(&value).unwrap();
            RObject::try_from(json).unwrap().sexp
        },
        Err(err) => {
            let err = format!("Failed to convert: {:?}", err);
            let msg = CString::new(err).unwrap();
            Rf_error(msg.as_ptr())
        },
    }
}
