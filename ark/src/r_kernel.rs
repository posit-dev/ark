/*
 * r_kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::socket::iopub::IOPubMessage;
use libc::{c_char, c_int, c_void};
use log::{debug, error, info, trace};
use std::ffi::CString;
use std::sync::mpsc::Sender;

#[link(name = "R", kind = "dylib")]
extern "C" {
    // TODO: the arg array doesn't seem to be passable in a type safe way, should just be a raw pointer
    fn Rf_initialize_R(ac: c_int, av: &[*const c_char]) -> i32;

    fn Rf_mainloop();

    /// Global indicating whether R is running as the main program (affects
    /// R_CStackStart)
    static mut R_running_as_main_program: c_int;

    /// Flag indicating whether this is an interactive session. R typically sets
    /// this when attached to a tty.
    static mut R_Interactive: c_int;

    /// Pointer to file receiving console input
    static mut R_Consolefile: *const c_void;

    /// Pointer to file receiving output
    static mut R_Outputfile: *const c_void;

    // TODO: type of buffer isn't necessary c_char
    static mut ptr_R_ReadConsole: unsafe extern "C" fn(*mut c_char, *mut c_char, i32, i32) -> i32;
}

pub struct RKernel {}

#[no_mangle]
pub extern "C" fn r_read_console(
    prompt: *mut c_char,
    _buf: *mut c_char,
    _buflen: i32,
    _hist: i32,
) -> i32 {
    unsafe {
        let r_prompt = CString::from_raw(prompt);
        trace!("R read console with prompt: {}", r_prompt.to_str().unwrap());
    }
    0
}

impl RKernel {
    pub fn start(sender: Sender<IOPubMessage>) {
        // TODO: Discover R locations and populate R_HOME, a prerequisite to
        // initializing R.
        //
        // Maybe add a command line option to specify the path to R_HOME directly?
        unsafe {
            let arg1 = CString::new("ark").unwrap();
            let arg2 = CString::new("--interactive").unwrap();
            let args = vec![arg1.as_ptr(), arg2.as_ptr()];
            R_running_as_main_program = 1;
            R_Interactive = 1;
            R_Consolefile = std::ptr::null();
            R_Outputfile = std::ptr::null();
            ptr_R_ReadConsole = r_read_console;
            Rf_initialize_R(args.len() as i32, &args);

            // Does not return
            Rf_mainloop();
        }
    }
}
