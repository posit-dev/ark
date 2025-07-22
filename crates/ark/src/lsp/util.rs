//
// util.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::os::raw::c_char;

use harp::object::RObject;
use libr::R_NilValue;
use libr::Rf_mkString;
use libr::SEXP;

/// Shows a message in the Positron frontend
#[harp::register]
pub unsafe extern "C-unwind" fn ps_log_error(message: SEXP) -> anyhow::Result<SEXP> {
    let message = RObject::view(message).to::<String>();
    if let Ok(message) = message {
        log::error!("{}", message);
    }

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_object_id(object: SEXP) -> anyhow::Result<SEXP> {
    let value = format!("{:p}", object);
    return Ok(Rf_mkString(value.as_ptr() as *const c_char));
}

#[cfg(test)]
pub(crate) fn test_path() -> (std::path::PathBuf, url::Url) {
    use std::path::PathBuf;

    let path = if cfg!(windows) {
        PathBuf::from(r"C:\test.R")
    } else {
        PathBuf::from("/test.R")
    };
    let uri = url::Url::from_file_path(&path).unwrap();

    (path, uri)
}
