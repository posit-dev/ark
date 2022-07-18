// 
// r_exec.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Once;

use extendr_api::*;
use libR_sys::*;

use crate::r_lock::rlock;

static INIT: Once = Once::new();
static mut PRECIOUS_LIST: SEXP = std::ptr::null_mut();

struct RObject {
    value: SEXP,
}

fn install<'a>(string: impl Into<&'a str>) -> SEXP {
    let value = string.into();
    let cstr = CString::new(value).expect("error constructing C string");
    unsafe { Rf_install(cstr.as_ptr() as *const c_char) }
}

struct RProtect {
    count: i32,
}

impl RProtect {

    pub fn new() -> Self {
        Self {
            count: 0
        }
    }

    pub fn add(&mut self, object: SEXP) -> SEXP {
        rlock! { Rf_protect(object) };
        self.count += 1;
        object
    }

}

impl Drop for RProtect {

    fn drop(&mut self) {
        rlock! { Rf_unprotect(self.count) };
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

trait RFunctionExt<T> {
    fn param(&mut self, name: &str, value: T) -> &mut Self;
    fn add(&mut self, value: T) -> &mut Self {
        self.param("", value)
    }
}

impl RFunctionExt<SEXP> for RFunction {

    fn param(&mut self, name: &str, value: SEXP) -> &mut Self {
        self.arguments.push(RArgument {
            name: name.to_string(),
            value: self.protect.add(value),
        });
        self
    }

}

impl RFunctionExt<Robj> for RFunction {

    fn param(&mut self, name: &str, value: Robj) -> &mut Self {
        unsafe { self.param(name, value.get()) }
    }

}

impl RFunctionExt<i32> for RFunction {

    fn param(&mut self, name: &str, value: i32) -> &mut Self {
        let value = rlock! { Rf_ScalarInteger(value) };
        self.param(name, value)
    }

}

impl RFunctionExt<&str> for RFunction {

    fn param(&mut self, name: &str, value: &str) -> &mut Self {
        
        let value = rlock! {
            let vector = self.protect.add(Rf_allocVector(STRSXP, 1));
            let element = Rf_mkCharLenCE(value.as_ptr() as *const i8, value.len() as i32, cetype_t_CE_UTF8);
            SET_STRING_ELT(vector, 0, element);
            vector
        };

        self.param(name, value)
    }

}

impl RFunctionExt<String> for RFunction {

    fn param(&mut self, name: &str, value: String) -> &mut Self {
        self.param(name, value.as_str())
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

    fn call(&mut self, protect: &mut RProtect) -> SEXP {
        rlock! { self.call_impl(protect) }
    }

    fn call_impl(&mut self, protect: &mut RProtect) -> SEXP { unsafe {

        // start building the call to be evaluated
        let lhs = if !self.package.is_empty() {
            self.protect.add(Rf_lang3(
                install(":::"),
                install(&*self.package),
                install(&*self.function)
            ))
        } else {
            install(&*self.function)
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
                SET_TAG(slot, install(&*argument.name));
            }
            slot = CDR(slot);
        }

        // now, wrap call in tryCatch
        let call = Rf_lang3(install("tryCatch"), call, install("identity"));
        SET_TAG(call, R_NilValue);
        SET_TAG(CDDR(call), install("error"));

        // evaluate the call
        let result = Rf_eval(call, R_BaseEnv);

        // TODO:
        // - check for errors?
        // - consider using a result type here?
        // - should we allow the caller to decide how errors are handled?

        // and return it
        protect.add(result);
        return result;

    } }

}

#[cfg(test)]
mod tests {

    use super::*;

    use crate::r_test;

    #[test]
    fn test_basic_function() { unsafe {

        r_test::start_r();

        // try adding some numbers
        let mut protect = RProtect::new();
        let result = RFunction::new("+")
            .add(Rf_ScalarInteger(2))
            .add(Rf_ScalarInteger(2))
            .call(&mut protect);

        // check the result
        assert!(Rf_isInteger(result) != 0);
        assert!(Rf_asInteger(result) == 4);

    } }

    #[test]
    fn test_utf8_strings() { unsafe {

        r_test::start_r();

        // try sending some UTF-8 strings to and from R
        let mut protect = RProtect::new();
        let result = RFunction::new("paste")
            .add("世界")
            .add("您好".to_string())
            .call(&mut protect);

        assert!(Rf_isString(result) != 0);

        let value = new_owned(result).as_str();
        assert!(value.is_some());
        assert!(value == Some("世界 您好"));

    }}

    #[test]
    fn test_named_arguments() { unsafe {

        r_test::start_r();

        let mut protect = RProtect::new();
        let result = RFunction::new("stats:::rnorm")
            .param("n", 1)
            .param("mean", 10)
            .param("sd", 0)
            .call(&mut protect);

        assert!(Rf_isNumeric(result) != 0);
        assert!(Rf_asInteger(result) == 10);

    }}

    #[test]
    fn test_threads() { unsafe {

        const N : i32 = 1000000;
        r_test::start_r();

        // Spawn a bunch of threads that try to interact with R.
        let mut handles : Vec<_> = Vec::new();
        for _i in 1..10 {
            let handle = std::thread::spawn(|| {
                for _j in 1..10 {
                    let result = rlock! {
                        let code = Rf_lang2(install("rnorm"), Rf_ScalarInteger(N));
                        Rf_eval(code, R_GlobalEnv)
                    };
                    assert!(Rf_length(result) == N);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("oh no");
        }

    }}

}

