//
// exec.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;
use std::mem::take;
use std::os::raw::c_void;

use anyhow::anyhow;
use libr::*;

use crate::call::RCall;
use crate::environment::R_ENVS;
use crate::error::Error;
use crate::error::Result;
use crate::line_ending::convert_line_endings;
use crate::line_ending::LineEnding;
use crate::modules::HARP_ENV;
use crate::object::r_list_get;
use crate::object::r_null_or_try_into;
use crate::object::RObject;
use crate::protect::RProtect;
use crate::r_string;
use crate::r_symbol;
use crate::utils::r_stringify;

pub enum ParseResult {
    Complete(SEXP),
    Incomplete(),
}

pub struct RFunction {
    pub call: RCall,
    is_namespaced: bool,
}

struct CallbackData<'a, F, T>
where
    F: FnOnce() -> T + 'a,
{
    res: &'a mut Option<harp::Result<T>>,
    closure: Option<F>,
}

impl RFunction {
    pub fn new(package: &str, function: &str) -> Self {
        Self::new_ext(package, function, false)
    }

    pub fn new_internal(package: &str, function: &str) -> Self {
        Self::new_ext(package, function, true)
    }

    pub fn new_inlined(function: impl Into<RObject>) -> Self {
        RFunction {
            call: RCall::new(function),
            is_namespaced: false,
        }
    }

    fn new_ext(package: &str, function: &str, internal: bool) -> Self {
        unsafe {
            let is_namespaced = !package.is_empty();

            let fun = if is_namespaced {
                let op = if internal { ":::" } else { "::" };
                Rf_lang3(r_symbol!(op), r_symbol!(package), r_symbol!(function))
            } else {
                r_symbol!(function)
            };
            let fun = RObject::new(fun);

            RFunction {
                call: RCall::new(fun),
                is_namespaced,
            }
        }
    }

    pub fn call(&mut self) -> Result<RObject> {
        // FIXME: Once we have ArkFunction (see
        // https://github.com/posit-dev/positron/issues/2324), we no longer need
        // this logic to call in global. This probably shouldn't be the default?
        let env = if self.is_namespaced {
            R_ENVS.base
        } else {
            R_ENVS.global
        };

        self.call_in(env)
    }

    pub fn call_in(&mut self, env: SEXP) -> Result<RObject> {
        let user_call = self.call.build();
        try_eval(user_call, env.into())
    }
}

/// Evaluate R code in a context protected from errors and longjumps
///
/// Calls `Rf_eval()` inside `try_catch()`.
pub fn try_eval(expr: RObject, env: RObject) -> crate::Result<RObject> {
    let res = try_catch(|| unsafe { Rf_eval(expr.sexp, env.sexp) });

    match res {
        // Convert to RObject
        Ok(value) => Ok(value.into()),
        Err(err) => match err {
            // Set correct expression. Can this be less verbose?
            Error::EvaluationError {
                code: _,
                message,
                class,
                r_trace,
            } => Err(Error::EvaluationError {
                code: Some(unsafe { r_stringify(expr.sexp, "\n")? }),
                message,
                class,
                r_trace,
            }),
            // Propagate as is
            _ => Err(err),
        },
    }
}

impl From<&str> for RFunction {
    fn from(function: &str) -> Self {
        RFunction::new("", function)
    }
}

impl From<String> for RFunction {
    fn from(function: String) -> Self {
        RFunction::new("", function.as_str())
    }
}

// NOTE: Having to import this trait cause a bit of friction during
// development. Can we do without?
pub trait RFunctionExt<T> {
    fn param(&mut self, name: &str, value: T) -> &mut Self;
    fn add(&mut self, value: T) -> &mut Self;
}

impl<T: Into<RObject>> RFunctionExt<Option<T>> for RFunction {
    fn param(&mut self, name: &str, value: Option<T>) -> &mut Self {
        if let Some(value) = value {
            self.call.param(name, value.into());
        }
        self
    }

    fn add(&mut self, value: Option<T>) -> &mut Self {
        if let Some(value) = value {
            self.call.add(value.into());
        }
        self
    }
}

impl<T: Into<RObject>> RFunctionExt<T> for RFunction {
    fn param(&mut self, name: &str, value: T) -> &mut Self {
        self.call.param(name, value);
        self
    }

    fn add(&mut self, value: T) -> &mut Self {
        self.call.add(value);
        self
    }
}

/// Run closure in a context protected from errors and longjumps
///
/// `try_catch()` runs a closure and captures any R-level errors with an R
/// backtrace. It calls the closure inside `top_level_exec()` to inherit from
/// its safety properties: insulating the closure from condition handlers and
/// converting any unexpected longjumps into a Rust error.
///
/// Two kinds of `harp::Error` are potentially returned:
/// - `EvaluationError` if an error was caught.
/// - `TopLevelExecError` if an unexpected longjump was caught.
///
/// NOTE: Rust objects with `drop()` methods should be stored outside the
/// `try_catch()` context. It's fine to longjump (e.g. throw an R error) over
/// a Rust stack as long as it doesn't contain destructors.
pub fn try_catch<'env, F, T>(fun: F) -> harp::Result<T>
where
    F: FnOnce() -> T,
    F: 'env,
{
    // Allocate stack memory for the result
    let mut res: Option<harp::Result<T>> = None;

    // Move function to the payload
    let mut callback_data = CallbackData {
        res: &mut res,
        closure: Some(fun),
    };
    let payload = &mut callback_data as *mut _ as *mut c_void;

    extern "C" fn callback<'env, F, T>(payload: *mut c_void)
    where
        F: FnOnce() -> T,
        F: 'env,
    {
        let data: &mut CallbackData<F, T> = unsafe { &mut *(payload as *mut CallbackData<F, T>) };

        // Move closure here so it can be called. Required since that's an `FnOnce`.
        let closure = take(&mut data.closure).unwrap();

        // Call closure and move the result to its stack space
        *(data.res) = Some(Ok(closure()));
    }

    extern "C" fn handler<'env, F, T>(err: SEXP, payload: *mut c_void)
    where
        F: FnOnce() -> T,
        F: 'env,
    {
        let data: &mut CallbackData<F, T> = unsafe { &mut *(payload as *mut CallbackData<F, T>) };

        // Run in lambda to collect errors more easily
        if let Err(err) = (|| -> harp::Result<()> {
            unsafe {
                let err = RFunction::new("", "try_catch_handler")
                    .add(err)
                    .call_in(HARP_ENV.unwrap())?;

                // Invariant of error slot: List of length 4 [message, call, class, trace],
                // with `trace` possibly an empty string.

                let message: String = RObject::view(r_list_get(err.sexp, 0)).try_into()?;

                let call: Option<String> =
                    r_null_or_try_into(RObject::view(r_list_get(err.sexp, 1)))?;

                let class: Option<Vec<String>> =
                    r_null_or_try_into(RObject::view(r_list_get(err.sexp, 2)))?;

                let r_trace: String = RObject::view(r_list_get(err.sexp, 3)).try_into()?;

                *(data.res) = Some(Err(Error::EvaluationError {
                    code: call,
                    message,
                    class,
                    r_trace,
                }));

                Ok(())
            }
        })() {
            *(data.res) = Some(Err(Error::Anyhow(anyhow!(
                "Internal error in `try_catch()`: {err:?}"
            ))))
        };

        let call = {
            // Create call to jump back to top-level-exec
            RFunction::new("", "invokeRestart")
                .add(String::from("abort"))
                .call
                .build()
                .sexp
        };

        // We've dropped our non-POD types and are ready to jump
        unsafe {
            libr::Rf_protect(call);
            libr::Rf_eval(call, R_ENVS.base);
        }
        unreachable!();
    }

    let longjump = top_level_exec(|| unsafe {
        libr::R_withCallingErrorHandler(
            Some(callback::<F, T>),
            payload,
            Some(handler::<F, T>),
            payload,
        );
    });

    res.unwrap_or_else(|| {
        // Return a `TopLevelExecError` if we end up here after an unexpected longjump
        if let Err(err) = longjump {
            Err(err)
        } else {
            Err(harp::Error::Anyhow(anyhow!("Unreachable")))
        }
    })
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
/// `top_level_exec()` is a low-level operator. Prefer using `try_catch()`
/// if possible:
///
/// - `try_catch()` uses a more robust strategy to catch R errors.
///
/// - `try_catch()` captures R-level and Rust-level backtraces at the R error site.
///
/// - With top-level-exec, if an unhandled R-level error does occur during a
///   top-level context, the error message is normally printed in the R console,
///   even if the calling code recovers from the failure. Since we turn off normal
///   error printing via the `show.error.messages` global option though, that
///   isn't normally the case in Ark.
///
/// NOTE: Rust objects with `drop()` methods should be stored outside the
/// `top_level_exec()` context. It's fine to longjump (e.g. throw an R error)
/// over a Rust stack as long as it doesn't contain destructors.
pub fn top_level_exec<'env, F, T>(fun: F) -> harp::Result<T>
where
    F: FnOnce() -> T,
    F: 'env,
{
    // Allocate stack memory for the result
    let mut res: Option<harp::Result<T>> = None;

    // Move function to the payload
    let mut callback_data = CallbackData {
        res: &mut res,
        closure: Some(fun),
    };
    let payload = &mut callback_data as *mut _ as *mut c_void;

    extern "C" fn callback<'env, F, T>(args: *mut c_void)
    where
        F: FnOnce() -> T,
        F: 'env,
    {
        let data: &mut CallbackData<F, T> = unsafe { &mut *(args as *mut CallbackData<F, T>) };

        // Move closure here so it can be called. Required since that's an `FnOnce`.
        let closure = take(&mut data.closure).unwrap();

        // Call closure and move the result to its stack space
        *(data.res) = Some(Ok(closure()));
    }

    unsafe { R_ToplevelExec(Some(callback::<F, T>), payload) };

    match res {
        Some(res) => res,
        None => {
            let mut err_buf = geterrmessage();

            if err_buf.len() > 0 {
                err_buf = format!("\nLikely caused by: {err_buf}");
            }

            Err(Error::TopLevelExecError {
                message: String::from(format!("Unexpected longjump{err_buf}")),
                backtrace: std::backtrace::Backtrace::capture(),
                span_trace: tracing_error::SpanTrace::capture(),
            })
        },
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

#[allow(non_upper_case_globals)]
pub unsafe fn r_parse_vector(code: &str) -> Result<ParseResult> {
    let mut ps: ParseStatus = ParseStatus_PARSE_NULL;
    let mut protect = RProtect::new();
    let r_code = r_string!(convert_line_endings(code, LineEnding::Posix), &mut protect);

    let result: RObject = try_catch(|| R_ParseVector(r_code, -1, &mut ps, R_NilValue).into())?;

    match ps {
        ParseStatus_PARSE_OK => Ok(ParseResult::Complete(result.sexp)),
        ParseStatus_PARSE_INCOMPLETE => Ok(ParseResult::Incomplete()),
        ParseStatus_PARSE_ERROR => Err(Error::ParseSyntaxError {
            message: CStr::from_ptr(libr::get(R_ParseErrorMsg).as_ptr())
                .to_string_lossy()
                .to_string(),
            line: libr::get(R_ParseError) as i32,
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

pub fn r_source(file: &str) -> crate::Result<()> {
    r_source_in(file, R_ENVS.base)
}

pub fn r_source_in(file: &str, env: SEXP) -> crate::Result<()> {
    RFunction::new("base", "sys.source")
        .param("file", file)
        .param("envir", env)
        .call()?;

    Ok(())
}

pub fn r_source_str(code: &str) -> crate::Result<()> {
    r_source_str_in(code, R_ENVS.base)
}

pub fn r_source_str_in(code: &str, env: impl Into<SEXP>) -> crate::Result<()> {
    let exprs = r_parse_exprs(code)?;
    harp::exec::r_source_exprs_in(exprs, env)?;
    Ok(())
}

pub fn r_source_exprs(exprs: impl Into<SEXP>) -> crate::Result<()> {
    r_source_exprs_in(exprs, R_ENVS.base)
}

pub fn r_source_exprs_in(exprs: impl Into<SEXP>, env: impl Into<SEXP>) -> crate::Result<()> {
    let exprs = exprs.into();
    let env = env.into();

    // `exprs` is an EXPRSXP and doesn't need to be quoted when passed as
    // literal argument. Only the R-level `eval()` function evaluates expression
    // vectors.
    RFunction::new("base", "source")
        .param("exprs", exprs)
        .param("local", env)
        .call()?;

    Ok(())
}

/// Returns an EXPRSXP vector
pub fn r_parse_exprs(code: &str) -> Result<RObject> {
    match unsafe { r_parse_vector(code)? } {
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

/// This uses the R-level function `parse()` to create the srcrefs
pub fn r_parse_exprs_with_srcrefs(code: &str) -> Result<RObject> {
    unsafe {
        let mut protect = RProtect::new();

        // Because `parse(text =)` doesn't allow `\r\n` even on Windows
        let code = convert_line_endings(code, LineEnding::Posix);
        let code = r_string!(code, protect);

        RFunction::new("base", "parse")
            .param("text", code)
            .param("keep.source", true)
            .call()
    }
}

/// Returns a single expression
pub fn r_parse(code: &str) -> Result<RObject> {
    unsafe {
        let exprs = r_parse_exprs(code)?;

        let n = Rf_xlength(*exprs);
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
    let _scope = crate::raii::RLocalSandbox::new();
    try_catch(f)
}

/// Unwrap Rust error and throw as R error
///
/// Takes a lambda returning a `Result`. On error, converts the Rust error
/// to a string with `Display` and throw it as an R error.
///
/// Usage: Call this at the boundary between an R stack and a Rust stack.
/// The simplest way is to call it first in a Rust function called from R
/// so that the whole Rust stack is encapsulated in the closure passed to
/// `r_unwrap()`.
///
/// All Rust objects in the current frame must have been dropped because
/// `r_unwrap()` causes a C longjump that bypasses destructors. UB occurs
/// if there are any pending drops on the stack (most likely memory leaking
/// but potentially unstability and panics too).
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
        Ok(out) => {
            // First check for interrupts since we might just have spent some
            // time in a sandbox
            unsafe {
                R_CheckUserInterrupt();
            }
            return out;
        },
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

    unsafe {
        // Now throw the error over the R stack
        Rf_errorcall(R_NilValue, R_CHAR(STRING_ELT(sexp_msg, 0)));
    }
}

/// Check that stack space is sufficient.
///
/// Optionally takes a size in bytes, otherwise let R decide if we're too
/// close to the limit. The latter case is useful for stopping recursive
/// algorithms from blowing the stack.
pub fn r_check_stack(size: Option<usize>) -> Result<()> {
    unsafe {
        let out = top_level_exec(|| {
            if let Some(size) = size {
                R_CheckStack2(size);
            } else {
                R_CheckStack();
            }
        });

        match out {
            Ok(_) => Ok(()),
            Err(err) => {
                // Reset error buffer because we detect stack overflows by
                // inspecting this buffer, see `peek_execute_response()`
                let _ = RFunction::new("base", "stop").call();

                // Convert TopLevelExecError to StackUsageError
                match err {
                    Error::TopLevelExecError {
                        message,
                        backtrace,
                        span_trace,
                    } => Err(Error::StackUsageError {
                        message,
                        backtrace,
                        span_trace,
                    }),
                    _ => unreachable!(),
                }
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
    use crate::utils::r_typeof;

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
    fn test_basic_function_error() {
        r_test! {
            let result = RFunction::from("+")
                .add(1)
                .add("")
                .call();

            assert_match!(result, Err(err) => {
                let msg = format!("{err}");
                let re = regex::Regex::new("R backtrace:\n(.|\n)*1L [+] \"\"").unwrap();
                assert!(re.is_match(&msg));
            });
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
            let ok: harp::Result<RObject> = try_catch(|| {
                Rf_ScalarInteger(42).into()
            });
            assert_match!(ok, Ok(value) => {
                assert_eq!(r_typeof(value.sexp), INTSXP as u32);
                assert_eq!(INTEGER_ELT(value.sexp, 0), 42);
            });

            // Error case
            let out = try_catch(|| unsafe {
                // This leaks
                let msg = CString::new("ouch").unwrap();
                Rf_error(msg.as_ptr());
            });

            assert_match!(out, Err(Error::EvaluationError { message, class, .. }) => {
                assert_eq!(message, "ouch");
                assert_eq!(class.unwrap(), ["simpleError", "error", "condition"]);
            });

        }
    }

    #[test]
    fn test_top_level_exec() {
        r_test! {
            let ok = top_level_exec(|| { 42 });
            assert_match!(ok, Ok(value) => {
                assert_eq!(value, 42);
            });

            // SAFETY: Rust allocations out of the top-level-exec context
            // NOTE: "my message" shows up in the test output. We might
            // want to quiet that by setting `show.error.messages` to `FALSE`.
            let msg = CString::new("my message").unwrap();
            let stop = CString::new("stop").unwrap();

            let out = top_level_exec(|| unsafe {
                let msg = Rf_protect(Rf_cons(Rf_mkString(msg.as_ptr()), R_NilValue));
                let call = Rf_protect(Rf_lcons(Rf_install(stop.as_ptr()), msg));
                Rf_eval(call, R_BaseEnv);
                unreachable!()
            });

            assert_match!(out, Err(Error::TopLevelExecError { message, backtrace: _ , span_trace: _}) => {
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
                    assert_eq!(Rf_xlength(call), 2);
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
            libr::set(R_DirtyImage, 2);
            let sym = r_symbol!("aaa");
            Rf_defineVar(sym, Rf_ScalarInteger(42), R_GlobalEnv);
            assert_eq!(libr::get(R_DirtyImage), 1);

            libr::set(R_DirtyImage, 2);
            Rf_setVar(sym, Rf_ScalarInteger(43), R_GlobalEnv);
            assert_eq!(libr::get(R_DirtyImage), 1);

            libr::set(R_DirtyImage, 2);
            r_envir_remove("aaa", R_GlobalEnv);
            assert_eq!(libr::get(R_DirtyImage), 1);
        }
    }

    #[test]
    fn test_r_unwrap() {
        r_test! {
            let out: Result<RObject> = try_catch(|| {
                r_unwrap(|| Err::<RObject, anyhow::Error>(anyhow::anyhow!("ouch")))
            });

            assert_match!(out, Err(Error::EvaluationError { message, class, .. }) => {
                assert_eq!(message, "ouch");
                assert_eq!(class.unwrap(), ["simpleError", "error", "condition"]);
            });
        }
    }
}
