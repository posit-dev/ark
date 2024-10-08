//
// string.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use libr::ParseStatus;
use libr::R_NaString;
use libr::R_NilValue;
use libr::R_ParseVector;
use libr::Rf_xlength;
use libr::EXPRSXP;
use libr::SEXP;
use libr::STRSXP;
use libr::VECTOR_ELT;

use crate::object::RObject;
use crate::protect::RProtect;
use crate::r_string;
use crate::utils::r_typeof;

// Given a quoted R string, decode it to get the string value.
pub unsafe fn r_string_decode(code: &str) -> Option<String> {
    // convert to R string
    let mut protect = RProtect::new();
    let code = r_string!(code, &mut protect);

    // parse into vector
    let mut ps: ParseStatus = 0;
    let result = protect.add(R_ParseVector(code, -1, &mut ps, R_NilValue));

    // check for string in result
    if r_typeof(result) == EXPRSXP {
        if Rf_xlength(result) != 0 {
            let value = VECTOR_ELT(result, 0);
            if r_typeof(value) == STRSXP {
                return RObject::view(value).to::<String>().ok();
            }
        }
    }

    None
}

pub fn r_is_string(x: SEXP) -> bool {
    unsafe { r_typeof(x) == STRSXP && Rf_xlength(x) == 1 && x != R_NaString }
}
