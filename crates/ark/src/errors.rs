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

use crate::interface::R_MAIN;

#[harp::register]
unsafe extern "C" fn ps_record_error(evalue: SEXP, traceback: SEXP) -> SEXP {
    let main = unsafe { R_MAIN.as_mut().unwrap() };

    // Convert to `RObject` for access to `try_from()` / `try_into()` methods.
    let evalue = RObject::new(evalue);
    let traceback = RObject::new(traceback);

    let evalue: String = unwrap!(evalue.try_into(), Err(error) => {
        warn!("Can't convert `evalue` to a Rust string: {}.", error);
        "".to_string()
    });

    let traceback: Vec<String> = unwrap!(traceback.try_into(), Err(error) => {
        warn!("Can't convert `traceback` to a Rust string vector: {}.", error);
        Vec::<String>::new()
    });

    main.error_occurred = true;
    main.error_evalue = evalue;
    main.error_traceback = traceback;

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
