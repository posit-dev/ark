use std::ffi;

// Implementations for entry points in `debug.c`

#[no_mangle]
pub extern "C" fn ark_print_rs(_x: libr::SEXP) -> *const ffi::c_char {
    let s = "Hello world!";
    ffi::CString::new(s).unwrap().into_raw()
}
