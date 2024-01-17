//
// session.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use libR_shim::*;
use libr::R_BaseEnv;
use libr::R_GlobalEnv;
use libr::R_NilValue;
use libr::R_PreserveObject;
use libr::R_Srcref;
use libr::Rf_ScalarInteger;
use libr::Rf_findVar;
use libr::Rf_getAttrib;
use libr::Rf_protect;
use libr::Rf_unprotect;
use libr::Rf_xlength;
use libr::INTEGER_ELT;
use libr::RDEBUG;
use libr::VECTOR_ELT;
use stdext::unwrap;

use crate::exec::r_parse;
use crate::exec::RFunction;
use crate::object::RObject;
use crate::protect::RProtect;
use crate::r_lang;
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

    unsafe {
        let ffi = r_try_eval_silent(NFRAME_CALL.unwrap_unchecked(), R_BaseEnv)?;
        let n_frame = IntegerVector::new(ffi)?;
        Ok(n_frame.get_unchecked_elt(0))
    }
}

pub fn r_sys_frame(n: std::ffi::c_int) -> crate::error::Result<SEXP> {
    unsafe {
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
    let _ = unsafe {
        (|| -> anyhow::Result<()> {
            let info = r_try_eval_silent(STACK_INFO_CALL.unwrap(), R_GlobalEnv)?;
            Rf_protect(info);

            let n: isize = Rf_xlength(info).try_into()?;

            for i in (0..n).rev() {
                let frame = VECTOR_ELT(info, i);

                if frame != R_NilValue {
                    out.push(frame.try_into()?);
                }
            }

            Rf_unprotect(1);
            Ok(())
        })()
    }?;

    // Add information from top-level frame and shift the source
    // information one frame up so it represents the frame's execution
    // state instead of its call site.
    let pointer = unwrap!(stack_pointer_frame(), Err(err) => {
        log::error!("Can't retrieve top-level frame: {err}");
        return Ok(out);
    });
    stack_shift(&mut out, pointer);

    return Ok(out);
}

fn stack_pointer_frame() -> anyhow::Result<FrameInfo> {
    unsafe {
        let mut srcref = R_Srcref;

        // Shouldn't happen but just to be safe
        if r_typeof(srcref) == VECSXP {
            srcref = VECTOR_ELT(srcref, 0);
        }

        let n = Rf_xlength(srcref);
        if r_typeof(srcref) != INTSXP || n < 4 {
            anyhow::bail!("Expected integer vector for srcref");
        }

        // The first field is sensitive to #line directives if they exist,
        // which we want to honour in order to jump to original files
        // rather than generated files.
        let line_idx = 0;

        // We need the `column` value rather than the `byte` value, so we
        // can index into a character. However the srcref documentation
        // allows a 4 elements vector when the bytes and column values are
        // the same. We account for this here.
        let col_idx = if n >= 5 { 4 } else { 1 };

        let line = INTEGER_ELT(srcref, line_idx);
        let column = INTEGER_ELT(srcref, col_idx);

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

// The call stack produced by `sys.calls()` includes file information for
// the call site, not for the execution state inside the frame. We fix this
// by shifting the source information one frame up, starting from the
// global frame pointer `R_SrcRef`.
fn stack_shift(stack: &mut Vec<FrameInfo>, pointer: FrameInfo) {
    // Shouldn't happen but just in case
    if stack.len() == 0 {
        stack.insert(0, pointer);
        return;
    }

    let mut current = pointer;

    for frame in stack.iter_mut() {
        let next = frame.clone();

        frame.file = current.file;
        frame.line = current.line;
        frame.column = current.column;

        current = next;
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
