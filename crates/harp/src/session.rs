//
// session.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use libR_sys::*;
use libc::c_int;

use crate::exec::r_parse;
use crate::exec::r_try_catch_any;
use crate::object::RObject;
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
static mut STACK_INFO_CALL: Option<SEXP> = None;

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

#[derive(Clone)]
pub struct FrameInfo {
    pub file: String,
    pub line: i64,
    pub column: i64,
}

impl TryFrom<SEXP> for FrameInfo {
    type Error = anyhow::Error;
    fn try_from(value: SEXP) -> Result<Self, Self::Error> {
        unsafe {
            let file = VECTOR_ELT(value, 0);
            let file = RObject::view(file).to::<String>()?;

            let line = VECTOR_ELT(value, 1);
            let line = RObject::view(line).to::<i32>()?;

            let column = VECTOR_ELT(value, 2);
            let column = RObject::view(column).to::<i32>()?;

            Ok(FrameInfo {
                file,
                line: line.try_into()?,
                column: column.try_into()?,
            })
        }
    }
}

pub fn r_stack_info() -> anyhow::Result<Vec<FrameInfo>> {
    let mut out: Vec<FrameInfo> = vec![];
    let mut protect = unsafe { RProtect::new() };

    let _ = r_lock!({
        r_try_catch_any(|| -> anyhow::Result<()> {
            let info = r_try_eval_silent(STACK_INFO_CALL.unwrap(), R_GlobalEnv)?;
            protect.add(info);

            let n: isize = Rf_length(info).try_into()?;
            out = Vec::with_capacity(n.try_into()?);

            for i in 0..n {
                let frame = VECTOR_ELT(info, i);
                out.push(frame.try_into()?);
            }

            Ok(())
        })
    })??;

    return Ok(out);
}

fn init_interface() {
    unsafe {
        let nframe_call = r_lang!(r_symbol!("sys.nframe"));
        R_PreserveObject(nframe_call);
        NFRAME_CALL = Some(nframe_call);

        let stack_info_call = *r_parse(".ps.debug.stackInfo()").unwrap();
        R_PreserveObject(stack_info_call);
        STACK_INFO_CALL = Some(stack_info_call);
    }
}
