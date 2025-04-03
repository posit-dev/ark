use std::sync::atomic::Ordering;

use libr::Rf_PrintValue;

use crate::interface::WRITE_CONSOLE_TO_STDOUT;

pub fn with_console_to_stdout(cb: impl FnOnce()) {
    let old = WRITE_CONSOLE_TO_STDOUT.swap(true, Ordering::SeqCst);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cb));

    WRITE_CONSOLE_TO_STDOUT.store(old, Ordering::SeqCst);
    if let Err(err) = result {
        std::panic::resume_unwind(err);
    }
}

// Implementations for entry points in `debug.c`

#[no_mangle]
pub extern "C" fn ark_print_rs(x: libr::SEXP) {
    with_console_to_stdout(|| {
        unsafe { Rf_PrintValue(x) };
    });
}
