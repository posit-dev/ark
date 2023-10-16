//
// exec.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;
use std::mem;
use std::os::raw::c_char;
use std::os::raw::c_int;
use std::os::raw::c_void;

use libR_sys::*;

use crate::error::Error;
use crate::error::Result;
use crate::object::RObject;
use crate::protect::RProtect;
use crate::r_string;
use crate::r_symbol;
use crate::utils::convert_line_endings;
use crate::utils::r_inherits;
use crate::utils::r_stringify;
use crate::utils::r_typeof;
use crate::utils::LineEnding;
use crate::vector::CharacterVector;
use crate::vector::Vector;

extern "C" {
    pub static mut R_PolledEvents: Option<unsafe extern "C" fn()>;
}

#[no_mangle]
pub extern "C" fn r_polled_events_disabled() {}

extern "C" {
    pub static R_ParseError: c_int;
    pub static R_ParseErrorMsg: [c_char; 256usize];
    pub static mut R_DirtyImage: ::std::os::raw::c_int;
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

pub struct RFunction {
    package: String,
    function: String,
    arguments: Vec<RArgument>,
}

pub trait RFunctionExt<T> {
    fn param(&mut self, name: &str, value: T) -> &mut Self;
    fn add(&mut self, value: T) -> &mut Self;
}

impl<T: Into<RObject>> RFunctionExt<Option<T>> for RFunction {
    fn param(&mut self, name: &str, value: Option<T>) -> &mut Self {
        if let Some(value) = value {
            self._add(name, value.into());
        }
        self
    }

    fn add(&mut self, value: Option<T>) -> &mut Self {
        if let Some(value) = value {
            self._add("", value.into());
        }
        self
    }
}

impl<T: Into<RObject>> RFunctionExt<T> for RFunction {
    fn param(&mut self, name: &str, value: T) -> &mut Self {
        let value: RObject = value.into();
        return self._add(name, value);
    }

    fn add(&mut self, value: T) -> &mut Self {
        let value: RObject = value.into();
        return self._add("", value);
    }
}

impl RFunction {
    pub fn new(package: &str, function: &str) -> Self {
        RFunction {
            package: package.to_string(),
            function: function.to_string(),
            arguments: Vec::new(),
        }
    }

    fn _add(&mut self, name: &str, value: RObject) -> &mut Self {
        self.arguments.push(RArgument {
            name: name.to_string(),
            value,
        });
        self
    }

    pub unsafe fn call(&mut self) -> Result<RObject> {
        let mut protect = RProtect::new();

        // start building the call to be evaluated
        let mut lhs = r_symbol!(self.function);
        if !self.package.is_empty() {
            lhs = protect.add(Rf_lang3(r_symbol!(":::"), r_symbol!(self.package), lhs));
        }

        // now, build the actual call to be evaluated
        let size = (1 + self.arguments.len()) as R_xlen_t;
        let call = protect.add(Rf_allocVector(LANGSXP, size));
        SET_TAG(call, R_NilValue);
        SETCAR(call, lhs);

        // append arguments to the call
        let mut slot = CDR(call);
        for argument in self.arguments.iter() {
            // quote language objects by default
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

        // now, wrap call in tryCatch, so that errors don't longjmp
        let try_catch = protect.add(Rf_lang3(
            r_symbol!("::"),
            r_symbol!("base"),
            r_symbol!("tryCatch"),
        ));
        let call = protect.add(Rf_lang4(
            try_catch,
            call,
            r_symbol!("identity"),
            r_symbol!("identity"),
        ));
        SET_TAG(call, R_NilValue);
        SET_TAG(CDDR(call), r_symbol!("error"));
        SET_TAG(CDDDR(call), r_symbol!("interrupt"));

        // evaluate the call
        let envir = if self.package.is_empty() {
            R_GlobalEnv
        } else {
            R_BaseEnv
        };
        let result = protect.add(Rf_eval(call, envir));

        if r_inherits(result, "error") {
            let code = r_stringify(call, "\n")?;
            let message = geterrmessage();
            return Err(Error::EvaluationError { code, message });
        }

        return Ok(RObject::new(result));
    }
}

impl From<&str> for RFunction {
    fn from(function: &str) -> Self {
        RFunction::new("", function)
    }
}

pub fn geterrmessage() -> String {
    // SAFETY: Returns pointer to static memory buffer owned by R.
    let buffer = unsafe { R_curErrorBuf() };

    // SAFETY: The aforementioned buffer is never null.
    let cstr = unsafe { CStr::from_ptr(buffer) };

    match cstr.to_str() {
        Ok(value) => return value.to_string(),
        Err(_) => return "".to_string(),
    }
}

/// Wrappers around R_tryCatch()
///
/// Takes a single closure that returns either a SEXP or `()`. If an R error is
/// thrown this returns a an RError in the Err variant, otherwise it returns the
/// result of the closure wrapped in an RObject.
///
/// The handler closure is not used per se, we just get the condition verbatim in the Err variant
///
/// Safety: the body of the closure should be as simple as possible because in the event
///         of an R error, R will jump and there is no rust unwinding, i.e. rust values
///         are not dropped. A good rule of thumb is to consider the body of the closure
///         as C code.
///
/// ```ignore
/// SEXP R_tryCatch(
///     SEXP (*body)(void *), void *bdata,
///     SEXP conds,
///     SEXP (*handler)(SEXP, void *), void *hdata),
///     void (*finally)(void*), void* fdata
/// )
/// ```
pub unsafe fn r_try_catch_finally<F, R, S, Finally>(
    mut fun: F,
    classes: S,
    mut finally: Finally,
) -> Result<R>
where
    F: FnMut() -> R,
    Finally: FnMut(),
    S: Into<CharacterVector>,
{
    // C function that is passed as `body`. The actual closure is passed as
    // void* data, along with the pointer to the result variable.
    extern "C" fn body_fn<R>(arg: *mut c_void) -> SEXP {
        let data: &mut ClosureData<R> = unsafe { &mut *(arg as *mut ClosureData<R>) };

        // Move result to its stack space
        *(data.res) = Some((data.closure)());

        // Return dummy SEXP value expected by `R_tryCatch()`
        unsafe { R_NilValue }
    }

    // Allocate stack memory for the output
    let mut res: Option<R> = None;

    let mut body_data = ClosureData {
        res: &mut res,
        closure: &mut fun,
    };

    // handler just returns the condition and sets success to false
    // to signal that an error was caught
    //
    // This is similar to doing tryCatch(<C code>, error = force) in R
    // except that we can handle the weird case where the code
    // succeeds but returns a an error object
    let mut success: bool = true;
    let success_ptr: *mut bool = &mut success;

    extern "C" fn handler_fn(condition: SEXP, arg: *mut c_void) -> SEXP {
        // signal that there was an error
        let success_ptr = arg as *mut bool;
        unsafe {
            *success_ptr = false;
        }

        // and return the R condition as is
        condition
    }

    // C function that is passed as `finally`
    // the actual closure is passed as a void* through arg
    extern "C" fn finally_fn(arg: *mut c_void) {
        // extract the "closure" from the void*
        let closure: &mut &mut dyn FnMut() = unsafe { mem::transmute(arg) };

        closure();
    }

    // The actual finally closure is passed as a void*
    let mut finally_data: &mut dyn FnMut() = &mut finally;
    let finally_data = &mut finally_data;

    let classes = classes.into();

    let result = R_tryCatch(
        Some(body_fn::<R>),
        &mut body_data as *mut _ as *mut c_void,
        *classes,
        Some(handler_fn),
        success_ptr as *mut c_void,
        Some(finally_fn),
        finally_data as *mut _ as *mut c_void,
    );

    match success {
        true => {
            // the call to tryCatch() was successful, so we return the result
            // as an RObject
            Ok(res.unwrap())
        },
        false => {
            // the call to tryCatch failed, so result is a condition
            // from which we can extract classes and message via a call to conditionMessage()
            let classes: Vec<String> =
                RObject::from(Rf_getAttrib(result, R_ClassSymbol)).try_into()?;

            let mut protect = RProtect::new();
            let call = protect.add(Rf_lang2(r_symbol!("conditionMessage"), result));

            // TODO: wrap the call to conditionMessage() in a tryCatch
            //       but this cannot be another call to r_try_catch_error()
            //       because it creates a recursion problem
            let message: Vec<String> = RObject::from(Rf_eval(call, R_BaseEnv)).try_into()?;

            Err(Error::TryCatchError { message, classes })
        },
    }
}

struct ClosureData<'a, R> {
    res: &'a mut Option<R>,
    closure: &'a mut dyn FnMut() -> R,
}

pub unsafe fn r_try_catch<F, R>(fun: F) -> Result<RObject>
where
    F: FnMut() -> R,
    RObject: From<R>,
{
    let out = r_try_catch_any(fun);
    out.map(|x| RObject::from(x))
}

pub unsafe fn r_try_catch_any<F, R>(fun: F) -> Result<R>
where
    F: FnMut() -> R,
{
    let vector = CharacterVector::create(["error"]);
    r_try_catch_finally(fun, vector, || {})
}

pub unsafe fn r_try_catch_classes<F, R, S>(fun: F, classes: S) -> Result<R>
where
    F: FnMut() -> R,
    S: Into<CharacterVector>,
{
    r_try_catch_finally(fun, classes, || {})
}

pub enum ParseResult {
    Complete(SEXP),
    Incomplete(),
}

#[allow(non_upper_case_globals)]
pub unsafe fn r_parse_vector(code: &str) -> Result<ParseResult> {
    let mut ps: ParseStatus = ParseStatus_PARSE_NULL;
    let mut protect = RProtect::new();
    let r_code = r_string!(convert_line_endings(code, LineEnding::Posix), &mut protect);

    let result = r_try_catch(|| R_ParseVector(r_code, -1, &mut ps, R_NilValue))?;

    match ps {
        ParseStatus_PARSE_OK => Ok(ParseResult::Complete(*result)),
        ParseStatus_PARSE_INCOMPLETE => Ok(ParseResult::Incomplete()),
        ParseStatus_PARSE_ERROR => Err(Error::ParseSyntaxError {
            message: CStr::from_ptr(R_ParseErrorMsg.as_ptr())
                .to_string_lossy()
                .to_string(),
            line: R_ParseError as i32,
        }),
        _ => {
            // should not get here
            Err(Error::ParseError {
                code: code.to_string(),
                message: String::from("Unknown parse error"),
            })
        },
    }
}

pub unsafe fn r_source(file: &str) -> Result<()> {
    let mut func = RFunction::new("base", "source");
    func.param("file", file);

    match func.call() {
        // Return value isn't meaningful here
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Returns an EXPRSXP vector
pub unsafe fn r_parse_exprs(code: &str) -> Result<RObject> {
    match r_parse_vector(code)? {
        ParseResult::Complete(x) => {
            return Ok(RObject::from(x));
        },
        ParseResult::Incomplete() => {
            return Err(Error::ParseError {
                code: code.to_string(),
                message: String::from("Incomplete code"),
            });
        },
    };
}

/// Returns a single expression
pub fn r_parse(code: &str) -> Result<RObject> {
    unsafe {
        let exprs = r_parse_exprs(code)?;

        let n = Rf_length(*exprs);
        if n != 1 {
            return Err(Error::ParseError {
                code: code.to_string(),
                message: String::from("Expected a single expression, got {n}"),
            });
        }

        let expr = VECTOR_ELT(*exprs, 0);
        Ok(expr.into())
    }
}

// TODO: Should probably run in a toplevel-exec. Tasks also need a timeout.
// This could be implemented with R interrupts but would require to
// unsafely jump over the Rust stack, unless we wrapped all R API functions
// to return an Option.
pub fn r_safely<'env, F, T>(f: F) -> T
where
    F: FnOnce() -> T,
    F: 'env,
    T: 'env,
{
    // NOTE: Keep this function a Plain Old Frame without Drop destructors

    let polled_events = unsafe { R_PolledEvents };
    let interrupts_suspended = unsafe { R_interrupts_suspended };
    unsafe {
        // Disable polled events in this scope.
        R_PolledEvents = Some(r_polled_events_disabled);

        // Disable interrupts in this scope.
        R_interrupts_suspended = 1;
    }

    // Execute the callback.
    let result = f();

    // Restore state
    // TODO: Needs unwind protection
    unsafe {
        R_interrupts_suspended = interrupts_suspended;
        R_PolledEvents = polled_events;
    }

    result
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;
    use crate::assert_match;
    use crate::r_test;
    use crate::utils::r_envir_remove;
    use crate::utils::r_is_null;

    #[test]
    fn test_basic_function() {
        r_test! {

            // try adding some numbers
            let result = RFunction::new("", "+")
                .add(2)
                .add(2)
                .call()
                .unwrap();

            // check the result
            assert!(Rf_isInteger(*result) != 0);
            assert!(Rf_asInteger(*result) == 4);

        }
    }

    #[test]
    fn test_utf8_strings() {
        r_test! {

            // try sending some UTF-8 strings to and from R
            let result = RFunction::new("base", "paste")
                .add("世界")
                .add("您好".to_string())
                .call()
                .unwrap();

            assert!(Rf_isString(*result) != 0);

            let value = TryInto::<String>::try_into(result);
            assert!(value.is_ok());
            if let Ok(value) = value {
                assert!(value == "世界 您好")
            }

        }
    }

    #[test]
    fn test_named_arguments() {
        r_test! {

            let result = RFunction::new("stats", "rnorm")
                .add(1.0)
                .param("mean", 10)
                .param("sd", 0)
                .call()
                .unwrap();

            assert!(Rf_isNumeric(*result) != 0);
            assert!(Rf_asInteger(*result) == 10);

        }
    }

    #[test]
    fn test_try_catch_error() {
        r_test! {

            // ok SEXP
            let ok = r_try_catch(|| {
                Rf_ScalarInteger(42)
            });
            assert_match!(ok, Ok(value) => {
                assert_eq!(r_typeof(*value), INTSXP as u32);
                assert_eq!(INTEGER_ELT(*value, 0), 42);
            });

            // ok void
            let void_ok = r_try_catch(|| {});
            assert_match!(void_ok, Ok(value) => {
                assert!(r_is_null(*value));
            });

            // ok something else, Vec<&str>
            let value = r_try_catch(|| {
                CharacterVector::create(["hello", "world"]).cast()
            });

            assert_match!(value, Ok(value) => {
                assert_eq!(r_typeof(*value), STRSXP);
                let value = CharacterVector::new(value);
                assert_match!(value, Ok(value) => {
                    assert_eq!(value, ["hello", "world"]);
                })
            });

            // error
            let out = r_try_catch(|| unsafe {
                let msg = CString::new("ouch").unwrap();
                Rf_error(msg.as_ptr());
            });

            assert_match!(out, Err(Error::TryCatchError { message, classes }) => {
                assert_eq!(message, ["ouch"]);
                assert_eq!(classes, ["simpleError", "error", "condition"]);
            });

        }
    }

    #[test]
    fn test_parse_vector() {
        r_test! {
            // complete
            assert_match!(
                r_parse_vector("force(42)"),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out), EXPRSXP as u32);

                    let call = VECTOR_ELT(out, 0);
                    assert_eq!(r_typeof(call), LANGSXP as u32);
                    assert_eq!(Rf_length(call), 2);
                    assert_eq!(CAR(call), r_symbol!("force"));

                    let arg = CADR(call);
                    assert_eq!(r_typeof(arg), REALSXP as u32);
                    assert_eq!(*REAL(arg), 42.0);
                }
            );

            // incomplete
            assert_match!(
                r_parse_vector("force(42"),
                Ok(ParseResult::Incomplete())
            );

            // error
            assert_match!(
                r_parse_vector("42 + _"),
                Err(_) => {}
            );

            // "normal" syntax error
            assert_match!(
                r_parse_vector("1+1\n*42"),
                Err(Error::ParseSyntaxError {message, line}) => {
                    assert!(message.contains("unexpected"));
                    assert_eq!(line, 2);
                }
            );

            // CRLF in the code string, like a file with CRLF line endings
            assert_match!(
                r_parse_vector("x<-\r\n1\r\npi"),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out), EXPRSXP as u32);
                    assert_eq!(r_stringify(out, "").unwrap(), "expression(x <- 1, pi)");
                }
            );

            // CRLF inside a string literal in the code
            assert_match!(
                r_parse_vector(r#"'a\r\nb'"#),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out), EXPRSXP as u32);
                    assert_eq!(r_stringify(out, "").unwrap(), r#"expression("a\r\nb")"#);
                }
            );
        }
    }

    #[test]
    fn test_dirty_image() {
        r_test! {
            R_DirtyImage = 2;
            let sym = r_symbol!("aaa");
            Rf_defineVar(sym, Rf_ScalarInteger(42), R_GlobalEnv);
            assert_eq!(R_DirtyImage, 1);

            R_DirtyImage = 2;
            Rf_setVar(sym, Rf_ScalarInteger(43), R_GlobalEnv);
            assert_eq!(R_DirtyImage, 1);

            R_DirtyImage = 2;
            r_envir_remove("aaa", R_GlobalEnv);
            assert_eq!(R_DirtyImage, 1);
        }
    }
}
