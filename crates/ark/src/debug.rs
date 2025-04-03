use std::ffi;
use std::sync::atomic::Ordering;

use harp::utils::r_str_to_owned_utf8_unchecked;
use libr::Rf_PrintValue;

use crate::interface::RMain;
use crate::interface::CAPTURE_CONSOLE_OUTPUT;

pub fn with_console_to_stdout(cb: impl FnOnce()) -> *const ffi::c_char {
    let old = CAPTURE_CONSOLE_OUTPUT.swap(true, Ordering::SeqCst);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cb));

    CAPTURE_CONSOLE_OUTPUT.store(old, Ordering::SeqCst);
    let out = std::mem::take(&mut RMain::get_mut().captured_output);

    if let Err(err) = result {
        std::panic::resume_unwind(err);
    }

    // Intentionally leaks, should only be used in the debugger
    ffi::CString::new(out).unwrap().into_raw()
}

// Implementations for entry points in `debug.c`

#[no_mangle]
pub extern "C" fn ark_print_rs(x: libr::SEXP) -> *const ffi::c_char {
    // TODO: protect against longjumps, print can dispatch

    with_console_to_stdout(|| {
        unsafe { Rf_PrintValue(x) };
    })
}

#[no_mangle]
pub extern "C" fn ark_display_value_rs(x: libr::SEXP) -> *const ffi::c_char {
    let value = unsafe {
        let kind = tidy_kind(libr::TYPEOF(x) as u32);
        let tag = format!("<{kind}>");

        match libr::TYPEOF(x) as u32 {
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
