//
// session.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use libR_sys::*;

use crate::utils::r_try_eval_silent;
use crate::vector::integer_vector::IntegerVector;
use crate::vector::Vector;

// Globals
static SESSION_INIT: Once = Once::new();
static mut NFRAME_CALL: usize = 0;

pub fn r_n_frame() -> crate::error::Result<i32> {
    SESSION_INIT.call_once(init_interface);

    unsafe {
        let ffi = r_try_eval_silent(NFRAME_CALL as SEXP, R_BaseEnv)?;
        let n_frame = IntegerVector::new(ffi)?;
        Ok(n_frame.get_unchecked_elt(0))
    }
}

fn init_interface() {
    unsafe {
        let nframe_call = crate::r_lang!(crate::r_symbol!("sys.nframe"));
        R_PreserveObject(nframe_call);
        NFRAME_CALL = nframe_call as usize;
    }
}
