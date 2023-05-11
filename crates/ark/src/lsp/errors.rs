//
// errors.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::geterrmessage;
use libR_sys::*;

use crate::kernel::R_ERROR_MESSAGE;
use crate::kernel::R_ERROR_OCCURRED;

#[harp::register]
unsafe extern "C" fn ps_error_handler() -> SEXP {
    R_ERROR_OCCURRED.store(true, std::sync::atomic::Ordering::Release);
    R_ERROR_MESSAGE.store(geterrmessage());
    R_NilValue
}
