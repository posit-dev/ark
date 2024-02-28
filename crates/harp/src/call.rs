//
// call.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use libr::*;

use crate::object::RObject;
use crate::protect::RProtect;
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
            let mut protect = RProtect::new();

            // Now, build the actual call to be evaluated
            let size = (1 + self.arguments.len()) as R_xlen_t;
            let call = protect.add(Rf_allocVector(LANGSXP, size));
            SET_TAG(call, R_NilValue);
            SETCAR(call, self.function.sexp);

            // Append arguments to the call
            let mut slot = CDR(call);
            for argument in self.arguments.iter() {
                // Quote language objects by default
                // FIXME: Shouldn't this be done by the caller?
                let mut sexp = argument.value.sexp;
                if matches!(r_typeof(sexp), LANGSXP | SYMSXP | EXPRSXP) {
                    let quote = protect.add(Rf_lang3(
                        r_symbol!("::"),
                        r_symbol!("base"),
                        r_symbol!("quote"),
                    ));
                    sexp = protect.add(Rf_lang2(quote, sexp));
                }

                SETCAR(slot, sexp);
                if !argument.name.is_empty() {
                    SET_TAG(slot, r_symbol!(argument.name));
                }

                slot = CDR(slot);
            }

            RObject::new(call)
        }
    }
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
