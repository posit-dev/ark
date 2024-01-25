//
// errors.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::object::RObject;
use harp::r_symbol;
use libr::R_GlobalEnv;
use libr::R_NilValue;
use libr::Rf_eval;
use libr::Rf_lcons;
use libr::SEXP;
use log::info;
use log::warn;
use stdext::unwrap;

use crate::interface::RMain;

#[harp::register]
unsafe extern "C" fn ps_record_error(evalue: SEXP, traceback: SEXP) -> anyhow::Result<SEXP> {
    let main = RMain::get_mut();

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
    main.error_message = evalue;
    main.error_traceback = traceback;

    Ok(R_NilValue)
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

#[harp::register]
unsafe extern "C" fn ps_rust_backtrace() -> anyhow::Result<SEXP> {
    let trace = std::backtrace::Backtrace::capture();
    let trace = format!("{trace}");
    Ok(*RObject::from(trace))
}
