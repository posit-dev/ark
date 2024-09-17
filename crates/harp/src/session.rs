//
// session.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use libr::*;
use stdext::unwrap;

use crate::environment::R_ENVS;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::modules::HARP_ENV;
use crate::object::RObject;
use crate::protect::RProtect;
use crate::r_lang;
use crate::r_symbol;
use crate::utils::r_typeof;
use crate::vector::integer_vector::IntegerVector;
use crate::vector::Vector;

// Globals
static SESSION_INIT: Once = Once::new();
static mut NFRAME_CALL: Option<SEXP> = None;
static mut SYS_CALLS_CALL: Option<SEXP> = None;
static mut SYS_FRAMES_CALL: Option<SEXP> = None;

pub fn r_n_frame() -> crate::Result<i32> {
    SESSION_INIT.call_once(init_interface);

    unsafe {
        let ffi = harp::try_eval_silent(NFRAME_CALL.unwrap_unchecked(), R_ENVS.base)?;
        let n_frame = IntegerVector::new(ffi)?;
        Ok(n_frame.get_unchecked(0).unwrap())
    }
}

pub fn r_sys_calls() -> crate::Result<RObject> {
    SESSION_INIT.call_once(init_interface);

    unsafe {
        Ok(harp::try_eval_silent(
            SYS_CALLS_CALL.unwrap_unchecked(),
            R_BaseEnv,
        )?)
    }
}

pub fn r_sys_frames() -> crate::Result<RObject> {
    SESSION_INIT.call_once(init_interface);

    unsafe {
        Ok(harp::try_eval_silent(
            SYS_FRAMES_CALL.unwrap_unchecked(),
            R_BaseEnv,
        )?)
    }
}

pub fn r_sys_functions() -> crate::Result<SEXP> {
    unsafe {
        let mut protect = RProtect::new();

        let n = r_n_frame()?;

        let out = Rf_allocVector(VECSXP, n as isize);
        protect.add(out);

        let fun = r_symbol!("sys.function");

        for i in 0..n {
            let mut protect = RProtect::new();

            let index = Rf_ScalarInteger(i + 1);
            protect.add(index);

            let call = r_lang!(fun, index);
            protect.add(call);

            SET_VECTOR_ELT(
                out,
                i as isize,
                harp::try_eval_silent(call, R_BaseEnv)?.sexp,
            );
        }

        Ok(out)
    }
}

pub fn r_sys_frame(n: std::ffi::c_int) -> crate::Result<RObject> {
    unsafe {
        let mut protect = RProtect::new();
        let n = protect.add(Rf_ScalarInteger(n));
        let call = protect.add(r_lang!(r_symbol!("sys.frame"), n));
        Ok(harp::try_eval_silent(call, R_BaseEnv)?)
    }
}

pub fn r_sys_call(n: std::ffi::c_int) -> crate::Result<RObject> {
    unsafe {
        let mut protect = RProtect::new();
        let n = protect.add(Rf_ScalarInteger(n));
        let call = protect.add(r_lang!(r_symbol!("sys.call"), n));
        Ok(harp::try_eval_silent(call, R_BaseEnv)?)
    }
}

pub fn r_env_is_browsed(env: SEXP) -> anyhow::Result<bool> {
    if r_typeof(env) != ENVSXP {
        anyhow::bail!("`env` must be an environment");
    }

    let browsed = unsafe { RDEBUG(env) };
    Ok(browsed != 0)
}

pub fn r_traceback() -> Vec<String> {
    let trace = RFunction::new("", ".ps.errors.traceback").call();

    match trace {
        Err(err) => {
            log::error!("Can't get traceback: {err:?}");
            vec![]
        },
        Ok(trace) => {
            unwrap!(Vec::<String>::try_from(trace), Err(err) => {
                log::error!("Can't convert traceback: {err:?}");
                vec![]
            })
        },
    }
}

pub fn r_format_traceback(calls: RObject) -> crate::Result<RObject> {
    RFunction::new("", "format_traceback")
        .add(calls)
        .call_in(unsafe { HARP_ENV.unwrap() })
}

fn init_interface() {
    unsafe {
        let nframe_call = r_lang!(r_symbol!("sys.nframe"));
        R_PreserveObject(nframe_call);
        NFRAME_CALL = Some(nframe_call);

        let sys_calls_call = r_lang!(r_symbol!("sys.calls"));
        R_PreserveObject(sys_calls_call);
        SYS_CALLS_CALL = Some(sys_calls_call);

        let sys_frames_call = r_lang!(r_symbol!("sys.frames"));
        R_PreserveObject(sys_frames_call);
        SYS_FRAMES_CALL = Some(sys_frames_call);
    }
}
