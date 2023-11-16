//
// exec.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;
use std::mem;
use std::mem::take;
use std::os::raw::c_char;
use std::os::raw::c_int;
use std::os::raw::c_void;

use libR_sys::*;

use crate::error::Error;
use crate::error::Result;
use crate::interrupts::RSandboxScope;
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
    fun: F,
    classes: S,
    finally: Finally,
) -> Result<R>
where
    F: FnOnce() -> R,
    Finally: FnOnce(),
    S: Into<CharacterVector>,
{
    // C function that is passed as `body`. The actual closure is passed as
    // void* data, along with the pointer to the result variable.
    extern "C" fn body_fn<F, R>(arg: *mut c_void) -> SEXP
    where
        F: FnOnce() -> R,
    {
        let data: &mut ClosureData<F, R> = unsafe { &mut *(arg as *mut ClosureData<F, R>) };

        // Move closure here so it can be called. Required since that's an `FnOnce`.
        let closure = take(&mut data.closure).unwrap();

        // Move result to its stack space
        *(data.res) = Some(closure());

        // Return dummy SEXP value expected by `R_tryCatch()`
        unsafe { R_NilValue }
    }

    // Allocate stack memory for the output
    let mut res: Option<R> = None;

    let mut body_data = ClosureData {
        res: &mut res,
        closure: Some(fun),
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
    extern "C" fn finally_fn<Finally>(arg: *mut c_void)
    where
        Finally: FnOnce(),
    {
        // Extract the "closure" from the void* and move it here
        let closure: &mut Option<Finally> = unsafe { mem::transmute(arg) };
        let closure = take(closure).unwrap();

        closure();
    }

    // The actual finally closure is passed as a void*
    let mut finally_data: Option<Finally> = Some(finally);
    let finally_data = &mut finally_data;

    let classes = classes.into();

    let result = R_tryCatch(
        Some(body_fn::<F, R>),
        &mut body_data as *mut _ as *mut c_void,
        *classes,
        Some(handler_fn),
        success_ptr as *mut c_void,
        Some(finally_fn::<Finally>),
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

struct ClosureData<'a, F, T>
where
    F: FnOnce() -> T + 'a,
{
    res: &'a mut Option<T>,
    closure: Option<F>,
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

/// Run closure inside top-level context
///
/// Top-level contexts are insulated from condition handlers (both calling
/// and exiting) on the R stack. This removes one source of side effects
/// and long jumps.
///
/// If a longjump does occur for any reason (including but not limited to R
/// errors), the caller is notified, in this case by an `Err` return value
/// of kind `TopLevelExecError`. The error message contains the contents of
/// the C-level error buffer. It might or might not be related to the cause
/// of the longjump. The error also carries a Rust backtrace.
///
/// Note that if an unhandled R-level error does occur during a top-level
/// context, the error message is normally printed in the R console, even
/// if the calling code recovers from the failure. Since we turn off normal
/// error printing via the `show.error.messages` global option though, that
/// isn't normally the case in Ark. That said, if errors are expected, it's
/// better to catch them with `r_try_catch()`.
pub fn r_top_level_exec<'env, F, T>(fun: F) -> Result<T>
where
    F: FnOnce() -> T,
    F: 'env,
{
    // Allocate stack memory for the result
    let mut res: Option<T> = None;

    // Because it's an `FnOnce`, the closure needs to be moved inside
    // `c_fn()` so it can be called. However we must also pass it via a
    // mutable borrow (void*), which prevents it from being moved. To work
    // around this, we wrap it in an `Option` that allows us to move it
    // with `take()`.
    let mut c_data = ClosureData {
        res: &mut res,
        closure: Some(fun),
    };
    let p_data = &mut c_data as *mut _ as *mut c_void;

    // C function that is passed as `fun`. The actual closure is passed via
    // void* data, along with the pointer to the result variable.
    extern "C" fn c_fn<'env, F, T>(arg: *mut c_void)
    where
        F: FnOnce() -> T,
        F: 'env,
    {
        let data: &mut ClosureData<F, T> = unsafe { &mut *(arg as *mut ClosureData<F, T>) };

        // Move closure here so it can be called. Required since that's an `FnOnce`.
        let closure = take(&mut data.closure).unwrap();

        // Call closure and move the result to its stack space
        *(data.res) = Some(closure());
    }

    let success = unsafe { R_ToplevelExec(Some(c_fn::<F, T>), p_data) };

    if success == 1 {
        Ok(res.unwrap())
    } else {
        Err(Error::TopLevelExecError {
            message: String::from(format!(
                "Unexpected longjump.\nLikely caused by: {}",
                geterrmessage()
            )),
            backtrace: std::backtrace::Backtrace::capture(),
        })
    }
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

// TODO: Tasks also need a timeout. This could be implemented with R
// interrupts but would require to unsafely jump over the Rust stack,
// unless we wrapped all R API functions to return an Option.
pub fn r_sandbox<'env, F, T>(f: F) -> Result<T>
where
    F: FnOnce() -> T,
    F: 'env,
    T: 'env,
{
    let _scope = RSandboxScope::new();
    r_top_level_exec(f)
}

/// Unwrap Rust error and throw as R error
///
/// Takes a lambda returning a `Result`. On error, converts the Rust error
/// to a string with `Display` and throw it as an R error.
///
/// Safety: This should only be used from within an R context frame such as
/// `.Call()` or `R_ExecWithCleanup()`.
pub fn r_unwrap<F, T, E>(f: F) -> T
where
    F: FnOnce() -> std::result::Result<T, E>,
    E: std::fmt::Display,
{
    let out = f();

    // Return early on success
    let msg = match out {
        Ok(out) => return out,
        Err(err) => format!("{err:}"),
    };

    // Move string to the R heap so it's protected there
    let robj_msg = RObject::from(msg);
    let sexp_msg = robj_msg.sexp;

    // We're about to drop our Rust wrapper so add the string to the
    // protection stack. We're relying on automatic unprotection after an
    // error, which requires `r_unwrap()` to be run within an R context
    // frame such as `.Call()` or `R_ExecWithCleanup()`.
    unsafe {
        Rf_protect(sexp_msg);
    }

    // Clear the Rust stack. We only need to drop `robj_msg` because `out`
    // was moved to `msg` and `msg` to `robj_msg` already.
    drop(robj_msg);

    // Now throw the error over the R stack
    unsafe {
        Rf_errorcall(R_NilValue, R_CHAR(STRING_ELT(sexp_msg, 0)));
    }
}

/// Check that stack space is sufficient.
///
/// Optionally takes a size in bytes, otherwise let R decide if we're too
/// close to the limit. The latter case is useful for stopping recursive
/// algorthims from blowing the stack.
pub fn r_check_stack(size: Option<usize>) -> Result<()> {
    unsafe {
        let out = r_top_level_exec(|| {
            if let Some(size) = size {
                R_CheckStack2(size);
            } else {
                R_CheckStack();
            }
        });

        // Reset error buffer because we detect stack overflows by
        // inspecting this buffer, see `peek_execute_response()`
        let _ = RFunction::new("base", "stop").call();

        // Convert TopLevelExecError to StackUsageError
        match out {
            Ok(_) => Ok(()),
            Err(err) => match err {
                Error::TopLevelExecError { message, backtrace } => {
                    Err(Error::StackUsageError { message, backtrace })
                },
                _ => unreachable!(),
            },
        }
    }
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
    fn test_top_level_exec() {
        r_test! {
            let ok = r_top_level_exec(|| { 42 });
            assert_match!(ok, Ok(value) => {
                assert_eq!(value, 42);
            });

            // SAFETY: Rust allocations out of the top-level-exec context
            // NOTE: "my message" shows up in the test output. We might
            // want to quiet that by setting `show.error.messages` to `FALSE`.
            let msg = CString::new("my message").unwrap();
            let stop = CString::new("stop").unwrap();

            let out = r_top_level_exec(|| unsafe {
                let msg = Rf_protect(Rf_cons(Rf_mkString(msg.as_ptr()), R_NilValue));
                let call = Rf_protect(Rf_lcons(Rf_install(stop.as_ptr()), msg));
                Rf_eval(call, R_BaseEnv);
                unreachable!()
            });

            assert_match!(out, Err(Error::TopLevelExecError { message, backtrace: _ }) => {
                assert!(message.contains("Unexpected longjump"));
                assert!(message.contains("my message"));
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

    #[test]
    fn test_r_unwrap() {
        r_test! {
            let out: Result<RObject> = r_try_catch(|| {
                r_unwrap(|| Err::<RObject, anyhow::Error>(anyhow::anyhow!("ouch")))
            });

            assert_match!(out, Err(Error::TryCatchError { message, classes }) => {
                assert_eq!(message, ["ouch"]);
                assert_eq!(classes, ["simpleError", "error", "condition"]);
            });
        }
    }
}
