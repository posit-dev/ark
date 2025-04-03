use std::ffi;
use std::sync::atomic::Ordering;

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
    with_console_to_stdout(|| {
        unsafe { Rf_PrintValue(x) };
    })
}
