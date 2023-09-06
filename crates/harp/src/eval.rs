//
// eval.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use libR_sys::*;

use crate::error::Error;
use crate::error::Result;
use crate::exec::geterrmessage;
use crate::exec::r_parse_exprs;
use crate::object::RObject;

pub struct RParseEvalOptions {
    pub forbid_function_calls: bool,
}

pub unsafe fn r_parse_eval0(code: &str) -> Result<RObject> {
    r_parse_eval(
        code,
        RParseEvalOptions {
            forbid_function_calls: false,
        },
    )
}

pub unsafe fn r_parse_eval(code: &str, options: RParseEvalOptions) -> Result<RObject> {
    // Forbid certain kinds of evaluation if requested.
    if options.forbid_function_calls && code.find('(').is_some() {
        return Err(Error::UnsafeEvaluationError(code.to_string()));
    }

    // Parse the provided code.
    let parsed_sexp = r_parse_exprs(code)?;

    // Evaluate the provided code.
    let mut value = R_NilValue;
    for i in 0..Rf_length(*parsed_sexp) {
        let expr = VECTOR_ELT(*parsed_sexp, i as isize);
        let mut errc: i32 = 0;
        value = R_tryEvalSilent(expr, R_GlobalEnv, &mut errc);
        if errc != 0 {
            return Err(Error::EvaluationError {
                code: code.to_string(),
                message: geterrmessage(),
            });
        }
    }

    Ok(RObject::new(value))
}
