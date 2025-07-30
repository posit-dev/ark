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

/// Create an absolute path from a file name.
/// File name is joined to the temp directory in a cross-platform way.
/// Returns both a `PathBuf` and a `Url`. Does not create the file.
#[cfg(test)]
pub(crate) fn test_path(file_name: &str) -> (std::path::PathBuf, url::Url) {
    let path = std::env::temp_dir().join(file_name);
    let uri = url::Url::from_file_path(&path).unwrap();
    (path, uri)
}
