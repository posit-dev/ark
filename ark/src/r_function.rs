// 
// r_function.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::os::raw::c_char;

use extendr_api::*;
use libR_sys::*;

struct RProtect {
    count: i32,
}

impl RProtect {

    pub fn new() -> Self {
        Self {
            count: 0
        }
    }

    pub fn add(&self, object: SEXP) -> SEXP {
        unsafe { Rf_protect(object) };
        object
    }

}

impl Drop for RProtect {

    fn drop(&mut self) {
        unsafe { Rf_unprotect(self.count) }
    }

}

struct RArgument {
    name: String,
    value: SEXP,
}

struct RFunction {
    package: String,
    function: String,
    arguments: Vec<RArgument>,
    protect: RProtect,
}

trait RFunctionParam<T> {
    fn param(&mut self, name: &str, value: T) -> &mut Self;
}

impl RFunctionParam<SEXP> for RFunction {

    fn param(&mut self, name: &str, value: SEXP) -> &mut Self {
        self.arguments.push(RArgument {
            name: name.to_string(),
            value: self.protect.add(value),
        });
        self
    }

}

impl RFunctionParam<Robj> for RFunction {

    fn param(&mut self, name: &str, value: Robj) -> &mut Self {
        unsafe { self.param(name, value.get()) }
    }

}

impl RFunctionParam<i32> for RFunction {

    fn param(&mut self, name: &str, value: i32) -> &mut Self {
        let value = unsafe { Rf_ScalarInteger(value) };
        self.param(name, value)
    }

}

impl RFunctionParam<&str> for RFunction {

    fn param(&mut self, name: &str, value: &str) -> &mut Self {
        
        let value = unsafe {
            let vector = self.protect.add(Rf_allocVector(STRSXP, 1));
            let element = Rf_mkCharLenCE(value.as_ptr() as *const i8, value.len() as i32, cetype_t_CE_UTF8);
            SET_STRING_ELT(vector, 0, element);
            vector
        };

        self.param(name, value)
    }

}

impl RFunctionParam<String> for RFunction {

    fn param(&mut self, name: &str, value: String) -> &mut Self {
        self.param(name, value.as_str())
    }
}

trait RFunctionAdd<T> {
    fn add(&mut self, value: T) -> &mut Self;
}

impl<T> RFunctionAdd<T> for RFunction where RFunction: RFunctionParam<T> {
    fn add(&mut self, value: T) -> &mut Self {
        self.param("", value)
    }
}

impl RFunction {

    fn new(value: &str) -> Self {

        let parts = value.split(":::").collect::<Vec<_>>();
        let (package, function) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("", value)
        };

        RFunction {
            package: package.to_string(),
            function: function.to_string(),
            arguments: Vec::new(),
            protect: RProtect::new(),
        }

    }

    fn call(&self) -> SEXP { unsafe {

        // start building the call to be evaluated
        let lhs = if !self.package.is_empty() {
            self.protect.add(Rf_lang3(
                Rf_install(":::".as_ptr() as *const c_char),
                Rf_install(self.package.as_ptr() as *const c_char),
                Rf_install(self.function.as_ptr() as *const c_char)
            ))
        } else {
            Rf_install(self.function.as_ptr() as *const c_char)
        };

        // now, build the actual call to be evaluated
        let size = (1 + self.arguments.len()) as R_xlen_t;
        let call = self.protect.add(Rf_allocVector(LANGSXP, size));
        SET_TAG(call, R_NilValue);
        SETCAR(call, lhs);

        // append arguments to the call
        let mut slot = CDR(call);
        for argument in self.arguments.iter() {
            SETCAR(slot, argument.value);
            if !argument.name.is_empty() {
                SET_TAG(slot, Rf_install(argument.name.as_ptr() as *const c_char));
            }
            slot = CDR(slot);
        }

        // evaluate the call
        let result = Rf_eval(call, R_BaseEnv);

        // and return it
        return result;

    } }

}

mod tests {
    use log::trace;

    use crate::r_test;

    use super::*;

    #[test]
    fn test_basic_function() { unsafe {

        r_test::start_r();

        // try adding some numbers
        let result = RFunction::new("+")
            .add(Rf_ScalarInteger(2))
            .add(Rf_ScalarInteger(2))
            .call();

        // check the result
        assert!(Rf_isInteger(result) != 0);
        assert!(Rf_asInteger(result) == 4);

    } }

    #[test]
    fn test_utf8_strings() { unsafe {

        r_test::start_r();

        // try sending some UTF-8 strings to and from R
        let result = RFunction::new("paste")
            .add("世界")
            .add("您好".to_string())
            .call();

        assert!(Rf_isString(result) != 0);

        let value = new_owned(result).as_str();
        assert!(value.is_some());
        assert!(value == Some("世界 您好"));

    }}

    #[test]
    fn test_named_arguments() { unsafe {

        r_test::start_r();

        let result = RFunction::new("stats:::rnorm")
            .param("n", 1)
            .param("mean", 10)
            .param("sd", 0)
            .call();

        assert!(Rf_isNumeric(result) != 0);
        assert!(Rf_asInteger(result) == 10);

    }}

}

