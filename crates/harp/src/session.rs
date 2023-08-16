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
use crate::object::RObject;
use crate::protect::RProtect;
use crate::r_lang;
use crate::r_lock;
use crate::r_symbol;
use crate::utils::r_normalize_path;
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
    pub name: String,
    pub file: String,
    pub line: i64,
    pub column: i64,
}

impl TryFrom<SEXP> for FrameInfo {
    type Error = anyhow::Error;
    fn try_from(value: SEXP) -> Result<Self, Self::Error> {
        unsafe {
            let mut i = 0;

            let name = VECTOR_ELT(value, i);
            let name = RObject::view(name).to::<String>()?;

            i += 1;
            let file = VECTOR_ELT(value, i);
            let file = RObject::view(file).to::<String>()?;

            i += 1;
            let line = VECTOR_ELT(value, i);
            let line = RObject::view(line).to::<i32>()?;

            i += 1;
            let column = VECTOR_ELT(value, i);
            let column = RObject::view(column).to::<i32>()?;

            Ok(FrameInfo {
                name,
                file,
                line: line.try_into()?,
                column: column.try_into()?,
            })
        }
    }
}

pub fn r_stack_info() -> anyhow::Result<Vec<FrameInfo>> {
    let mut out: Vec<FrameInfo> = vec![];

    // FIXME: It's better not to use `r_try_catch()` here because it adds
    // frames to the stack. Should wrap in a top-level-exec instead.
    let _ = r_lock!({
        (|| -> anyhow::Result<()> {
            let info = r_try_eval_silent(STACK_INFO_CALL.unwrap(), R_GlobalEnv)?;
            Rf_protect(info);

            // Add top-level frame
            // TODO: Shift srcrefs across the stack
            match stack_pointer_frame() {
                Ok(top) => out.push(top),
                Err(err) => log::error!("Can't retrieve top-level frame: {err}"),
            }

            let n: isize = Rf_length(info).try_into()?;

            for i in (0..n).rev() {
                let frame = VECTOR_ELT(info, i);

                if frame != R_NilValue {
                    out.push(frame.try_into()?);
                }
            }

            Rf_unprotect(1);
            Ok(())
        })()
    })?;

    return Ok(out);
}

fn stack_pointer_frame() -> anyhow::Result<FrameInfo> {
    unsafe {
        let mut srcref = R_Srcref;

        // Shouldn't happen but just to be safe
        if r_typeof(srcref) == VECSXP {
            srcref = VECTOR_ELT(srcref, 0);
        }

        if r_typeof(srcref) != INTSXP || Rf_length(srcref) < 5 {
            anyhow::bail!("Expected integer vector for srcref");
        }

        let line = INTEGER_ELT(srcref, 0);
        let column = INTEGER_ELT(srcref, 4);

        let srcfile: RObject = Rf_getAttrib(srcref, r_symbol!("srcfile")).into();

        if r_typeof(srcfile.sexp) != ENVSXP {
            anyhow::bail!("Expected environment for srcfile");
        }

        let file: RObject = Rf_findVar(r_symbol!("filename"), srcfile.sexp).into();
        let file = r_normalize_path(file)?;

        Ok(FrameInfo {
            name: String::from("<current>"),
            file,
            line: line.into(),
            column: column.into(),
        })
    }
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
