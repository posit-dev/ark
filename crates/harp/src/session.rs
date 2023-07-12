//
// session.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use libR_sys::*;
use libc::c_int;

use crate::protect::RProtect;
use crate::r_lang;
use crate::r_lock;
use crate::r_symbol;
use crate::utils::r_try_eval_silent;
use crate::utils::r_typeof;
use crate::vector::integer_vector::IntegerVector;
use crate::vector::Vector;

// Globals
static SESSION_INIT: Once = Once::new();
static mut NFRAME_CALL: Option<SEXP> = None;

pub fn r_n_frame() -> crate::error::Result<i32> {
    SESSION_INIT.call_once(init_interface);

    r_lock! {
        let ffi = r_try_eval_silent(NFRAME_CALL.unwrap_unchecked(), R_BaseEnv)?;
        let n_frame = IntegerVector::new(ffi)?;
        Ok(n_frame.get_unchecked_elt(0))
    }
}

pub fn r_sys_frame(n: c_int) -> crate::error::Result<SEXP> {
    r_lock! {
        let mut protect = RProtect::new();
        let n = protect.add(Rf_ScalarInteger(n));
        let call = protect.add(r_lang!(r_symbol!("sys.frame"), n));
        Ok(r_try_eval_silent(call, R_BaseEnv)?)
    }
}

pub fn r_env_is_browsed(env: SEXP) -> anyhow::Result<bool> {
    if r_typeof(env) != ENVSXP {
        anyhow::bail!("`env` must be an environment");
    }

    let browsed = unsafe { RDEBUG(env) };
    Ok(browsed != 0)
}

fn init_interface() {
    unsafe {
        let nframe_call = r_lang!(r_symbol!("sys.nframe"));
        R_PreserveObject(nframe_call);
        NFRAME_CALL = Some(nframe_call);
    }
}
