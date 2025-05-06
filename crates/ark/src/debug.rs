use std::ffi;
use std::sync::atomic::Ordering;

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::utils::r_str_to_owned_utf8_unchecked;
use harp::utils::r_typeof;

use crate::interface::RMain;
use crate::interface::CAPTURE_CONSOLE_OUTPUT;

// To ensure the compiler includes the C entry points in `debug.c` in the binary,
// we store function pointers in global variables that are declared "used" (even
// though we never actually use them). The compiler is able to follow the chain
// of dependency from these variables to the C functions and ultimately their
// Rust implementations defined below.
extern "C" {
    fn ark_print(x: libr::SEXP) -> *const ffi::c_char;
    fn ark_inspect(x: libr::SEXP) -> *const ffi::c_char;
    fn ark_trace_back() -> *const ffi::c_char;
    fn ark_display_value(x: libr::SEXP) -> *const ffi::c_char;
}

#[used]
static _ARK_PRINT: unsafe extern "C" fn(x: libr::SEXP) -> *const ffi::c_char = ark_print;
#[used]
static _ARK_INSPECT: unsafe extern "C" fn(x: libr::SEXP) -> *const ffi::c_char = ark_inspect;
#[used]
static _ARK_TRACE_BACK: unsafe extern "C" fn() -> *const ffi::c_char = ark_trace_back;
#[used]
static _ARK_DISPLAY_VALUE: unsafe extern "C" fn(x: libr::SEXP) -> *const ffi::c_char =
    ark_display_value;

// Implementations for entry points in `debug.c`.

#[no_mangle]
pub extern "C-unwind" fn ark_print_rs(x: libr::SEXP) -> *const ffi::c_char {
    capture_console_output(|| {
        unsafe { libr::Rf_PrintValue(x) };
    })
}

/// Inspect structure of R object
///
/// Uses lobstr's `sxp()` function because libr can't find `R_inspect()`.
/// It's an `attribute_hidden` function but since the symbol is visible
/// on macOS (and you can call it in the debugger) I would have expected
/// libr to be able to find it.
///
/// Requires lldb setting:
///
/// ```text
/// settings set escape-non-printables false
/// ```
#[no_mangle]
pub extern "C-unwind" fn ark_inspect_rs(x: libr::SEXP) -> *const ffi::c_char {
    capture_console_output(|| {
        // TODO: Should use C callable when implemented as that would avoid
        // messing with namedness and refcounts:
        // https://github.com/r-lib/lobstr/issues/77
        let out = RFunction::new("lobstr", "sxp").add(x).call().unwrap();
        unsafe { libr::Rf_PrintValue(out.sexp) };
    })
}

/// Print backtrace via rlang
///
/// Requires lldb setting:
///
/// ```text
/// settings set escape-non-printables false
/// ```
#[no_mangle]
pub extern "C-unwind" fn ark_trace_back_rs() -> *const ffi::c_char {
    capture_console_output(|| {
        // https://github.com/r-lib/rlang/issues/1059
        unsafe {
            let fun =
                get_c_callable_int(c"rlang".as_ptr(), c"rlang_print_backtrace".as_ptr()).unwrap();
            fun(1);
        };
    })
}

#[no_mangle]
pub extern "C-unwind" fn ark_display_value_rs(x: libr::SEXP) -> *const ffi::c_char {
    let value = unsafe {
        let kind = tidy_kind(r_typeof(x));
        let tag = format!("<{kind}>");

        match r_typeof(x) {
            libr::SYMSXP => format!(
                "{tag} ({name})",
                name = r_str_to_owned_utf8_unchecked(libr::PRINTNAME(x))
            ),
            // TODO: Show some values (not with ALTREP objects as that could
            // materialise or cause side effects)
            libr::LGLSXP |
            libr::INTSXP |
            libr::REALSXP |
            libr::CPLXSXP |
            libr::RAWSXP |
            libr::STRSXP |
            libr::VECSXP |
            libr::SPECIALSXP |
            libr::BUILTINSXP |
            libr::PROMSXP => {
                format!("{tag} [{len}]", len = libr::Rf_xlength(x))
            },

            _ => tag,
        }
    };

    ffi::CString::new(value).unwrap().into_raw()
}

pub fn tidy_kind(kind: libr::SEXPTYPE) -> &'static str {
    match kind {
        libr::NILSXP => "null",
        libr::SYMSXP => "sym",
        libr::LISTSXP => "list",
        libr::CLOSXP => "fn",
        libr::ENVSXP => "env",
        libr::PROMSXP => "prom",
        libr::LANGSXP => "call",
        libr::SPECIALSXP => "special",
        libr::BUILTINSXP => "builtin",
        libr::CHARSXP => "char",
        libr::LGLSXP => "lgl",
        libr::INTSXP => "int",
        libr::REALSXP => "dbl",
        libr::CPLXSXP => "cpl",
        libr::STRSXP => "chr",
        libr::DOTSXP => "dots",
        libr::ANYSXP => "any",
        libr::VECSXP => "list",
        libr::EXPRSXP => "expr",
        libr::BCODESXP => "bcode",
        libr::EXTPTRSXP => "extptr",
        libr::WEAKREFSXP => "weakref",
        libr::RAWSXP => "raw",
        libr::S4SXP => "s4",
        libr::NEWSXP => "new",
        libr::FREESXP => "free",
        libr::FUNSXP => "fun",
        _ => "unknown",
    }
}

/// Run closure and capture its console output.
///
/// Useful for debugging. For instance you can use this to call code from the C
/// debugger's interpreter. Output from stdout and stderr is returned instead of
/// being sent over IOPub.
///
/// The closure is run in a `harp::try_catch()` context to prevent R errors and
/// other C longjumps from collapsing the debugging context. If a Rust panic
/// occurs however, it is propagated as normal.
///
/// Note that the resulting string is stored on the Rust heap and never freed.
/// This should only be used in a debugging context where leaking is not an
/// issue.
pub fn capture_console_output(cb: impl FnOnce()) -> *const ffi::c_char {
    let old = CAPTURE_CONSOLE_OUTPUT.swap(true, Ordering::SeqCst);

    // We protect from panics to correctly restore `CAPTURE_CONSOLE_OUTPUT`'s
    // state. The panic is resumed right after.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| harp::try_catch(cb)));

    CAPTURE_CONSOLE_OUTPUT.store(old, Ordering::SeqCst);
    let mut out = std::mem::take(&mut RMain::get_mut().captured_output);

    // Unwrap catch-unwind's result and resume panic if needed
    let result = match result {
        Ok(res) => res,
        Err(err) => {
            std::panic::resume_unwind(err);
        },
    };

    // Unwrap try-catch's result
    if let Err(err) = result {
        out = format!("{out}\nUnexpected longjump in `capture_console_output()`: {err:?}");
    }

    // Intentionally leaks, should only be used in the debugger
    ffi::CString::new(out).unwrap().into_raw()
}

// Cast `DL_FUNC` to correct function type
fn get_c_callable_int(
    pkg: *const std::ffi::c_char,
    fun: *const std::ffi::c_char,
) -> Option<unsafe extern "C-unwind" fn(std::ffi::c_int) -> *mut std::ffi::c_void> {
    unsafe {
        std::mem::transmute::<
            Option<unsafe extern "C-unwind" fn() -> *mut std::ffi::c_void>,
            Option<unsafe extern "C-unwind" fn(std::ffi::c_int) -> *mut std::ffi::c_void>,
        >(libr::R_GetCCallable(pkg, fun))
    }
}
