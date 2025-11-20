//
// errors.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::wire::exception::Exception;
use harp::exec::r_peek_error_buffer;
use harp::exec::RE_STACK_OVERFLOW;
use harp::object::RObject;
use harp::r_symbol;
use harp::session::r_format_traceback;
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
unsafe extern "C-unwind" fn ps_record_error(evalue: SEXP, traceback: SEXP) -> anyhow::Result<SEXP> {
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

    main.last_error = Some(
        // We don't fill out `ename` with anything meaningful because typically
        // R errors don't have names. We could consider using the condition class
        // here, which r-lib/tidyverse packages have been using more heavily.
        Exception {
            ename: String::from(""),
            evalue,
            traceback,
        },
    );

    Ok(R_NilValue)
}

#[harp::register]
unsafe extern "C-unwind" fn ps_format_traceback(calls: SEXP) -> anyhow::Result<SEXP> {
    Ok(r_format_traceback(calls.into())?.sexp)
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
unsafe extern "C-unwind" fn ps_rust_backtrace() -> anyhow::Result<SEXP> {
    let trace = std::backtrace::Backtrace::force_capture();
    let trace = format!("{trace}");
    Ok(*RObject::from(trace))
}

pub(crate) fn stack_overflow_occurred() -> bool {
    // Error handlers are not called on stack overflow so the error flag
    // isn't set. Instead we detect stack overflows by peeking at the error
    // buffer. The message is explicitly not translated to save stack space
    // so the matching should be reliable.
    let err_buf = r_peek_error_buffer();
    RE_STACK_OVERFLOW.is_match(&err_buf)
}
