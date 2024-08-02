//
// call.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use libr::*;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::modules::HARP_ENV;
use crate::object::RObject;
use crate::r_symbol;
use crate::utils::r_typeof;

pub struct RCall {
    function: RObject,
    arguments: Vec<RArgument>,
}

impl RCall {
    pub fn new(function: impl Into<RObject>) -> Self {
        Self {
            function: function.into(),
            arguments: Vec::new(),
        }
    }

    pub fn param(&mut self, name: &str, value: impl Into<RObject>) -> &mut Self {
        self.arguments.push(RArgument {
            name: name.to_string(),
            value: value.into(),
        });
        self
    }

    pub fn add(&mut self, value: impl Into<RObject>) -> &mut Self {
        self.param("", value)
    }

    pub fn build(&self) -> RObject {
        unsafe {
            let call = RObject::new(Rf_lcons(self.function.sexp, R_NilValue));
            let mut tail = call.sexp;

            // Append arguments to the call
            for argument in self.arguments.iter() {
                SETCDR(tail, Rf_cons(argument.value.sexp, R_NilValue));

                tail = CDR(tail);
                if !argument.name.is_empty() {
                    SET_TAG(tail, r_symbol!(argument.name));
                }
            }

            call
        }
    }
}

pub fn r_expr_quote(x: impl Into<SEXP>) -> RObject {
    let x = x.into();
    match r_typeof(x) {
        SYMSXP | LANGSXP => return RFunction::new("base", "quote").add(x).call.build(),
        _ => return x.into(),
    }
}

pub fn r_expr_deparse(x: SEXP) -> harp::Result<String> {
    let x = RFunction::from("expr_deparse")
        .add(r_expr_quote(x))
        .call_in(unsafe { HARP_ENV.unwrap() })?;

    let x = String::try_from(x)?;

    Ok(x)
}

pub struct RArgument {
    pub name: String,
    pub value: RObject,
}

impl RArgument {
    pub fn new(name: &str, value: RObject) -> Self {
        Self {
            name: name.to_string(),
            value,
        }
    }
}
