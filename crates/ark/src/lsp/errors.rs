//
// errors.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::object::RObject;
use harp::r_symbol;
use libR_sys::*;
use log::info;
use log::warn;
use stdext::unwrap;

use crate::kernel::R_ERROR_EVALUE;
use crate::kernel::R_ERROR_OCCURRED;
use crate::kernel::R_ERROR_TRACEBACK;

#[harp::register]
unsafe extern "C" fn ps_record_error(evalue: SEXP, traceback: SEXP) -> SEXP {
    // TODO: Add `try_from()` methods for `SEXP`?
    // Convert to `RObject` for access to `try_from()` methods.
    let evalue = RObject::new(evalue);
    let traceback = RObject::new(traceback);

    let evalue = unwrap!(String::try_from(evalue), Err(error) => {
        warn!("Can't convert `evalue` to a Rust string: {}.", error);
        "".to_string()
    });

    let traceback = unwrap!(Vec::<String>::try_from(traceback), Err(error) => {
        warn!("Can't convert `traceback` to a Rust string vector: {}.", error);
        Vec::<String>::new()
    });

    R_ERROR_OCCURRED.store(true, std::sync::atomic::Ordering::Release);
    R_ERROR_EVALUE.store(evalue);
    R_ERROR_TRACEBACK.store(traceback);

    R_NilValue
}

pub unsafe fn initialize() {
    // Must be called after the public Positron function environment is set up
    info!("Initializing global error handler");

    let call = RObject::new(Rf_lcons(
        r_symbol!(".ps.errors.initializeGlobalErrorHandler"),
        R_NilValue,
    ));

    Rf_eval(*call, R_GlobalEnv);
}
