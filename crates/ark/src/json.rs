//
// json.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use harp::object::RObject;
use libR_sys::SEXP;

/// Convenience method to convert a JSON object to a string
#[harp::register]
pub unsafe extern "C" fn ps_to_json(obj: SEXP) -> anyhow::Result<SEXP> {
    let obj = RObject::view(obj);

    // Convert the object to a JSON value; this is the core serialization step
    let val = serde_json::Value::try_from(obj)?;

    // Format the JSON value as a string for display
    let json = serde_json::to_string_pretty(&val)?;

    // Create an R string from the JSON string
    let robj = RObject::try_from(json)?;
    Ok(robj.sexp)
}
