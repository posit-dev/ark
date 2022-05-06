/*
 * r_interface.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r_kernel::{RKernel, RKernelInfo};
use crate::r_request::RRequest;
use amalthea::socket::iopub::IOPubMessage;
use libc::{c_char, c_int};
use log::{debug, trace, warn};
use std::ffi::{CStr, CString};
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, Once};
use std::thread;

// --- Globals ---
// These values must be global in order for them to be accessible from R
// callbacks, which do not have a facility for passing or returning context.

/// The global R kernel state
pub static mut KERNEL: Option<Arc<Mutex<RKernel>>> = None;

/// A channel that sends prompts from R to the kernel
static mut RPROMPT_SEND: Option<Mutex<Sender<String>>> = None;

/// A channel that receives console input from the kernel and sends it to R;
/// sending empty input (None) tells R to shut down
static mut CONSOLE_RECV: Option<Mutex<Receiver<Option<String>>>> = None;

/// Ensures that the kernel is only ever initialized once
static INIT: Once = Once::new();

/// Invoked by R to read console input from the user.
///
/// * `prompt` - The prompt shown to the user
/// * `buf`    - Pointer to buffer to receive the user's input (type `CONSOLE_BUFFER_CHAR`)
/// * `buflen` - Size of the buffer to receiver user's input
/// * `hist`   - Whether to add the input to the history (1) or not (0)
///
#[no_mangle]
pub extern "C" fn r_read_console(
    prompt: *const c_char,
    buf: *mut u8,
    buflen: c_int,
    _hist: c_int,
) -> i32 {
    let r_prompt = unsafe { CStr::from_ptr(prompt) };
    debug!("R prompt: {}", r_prompt.to_str().unwrap());

    // If the prompt begins with "Save workspace", respond with (n)
    if r_prompt.to_str().unwrap().starts_with("Save workspace") {
        let n = CString::new("n\n").unwrap();
        unsafe {
            libc::strcpy(buf as *mut i8, n.as_ptr());
        }
        return 1;
    }

    // TODO: if R prompt is +, we need to tell the user their input is incomplete
    let mutex = unsafe { RPROMPT_SEND.as_ref().unwrap() };
    let sender = mutex.lock().unwrap();
    sender
        .send(r_prompt.to_string_lossy().into_owned())
        .unwrap();

    let mutex = unsafe { CONSOLE_RECV.as_ref().unwrap() };
    let recv = mutex.lock().unwrap();
    if let Some(mut input) = recv.recv().unwrap() {
        trace!("Sending input to R: '{}'", input);
        input.push_str("\n");
        if input.len() < buflen.try_into().unwrap() {
            let src = CString::new(input).unwrap();
            unsafe {
                libc::strcpy(buf as *mut i8, src.as_ptr());
            }
        } else {
            // Input doesn't fit in buffer
            // TODO: need to allow next call to read the buffer
            return 1;
        }
    } else {
        trace!("Exiting R from console reader");
        return 0;
    }
    // Nonzero return values indicate the end of input and cause R to exit
    1
}

#[no_mangle]
pub extern "C" fn r_write_console(buf: *const c_char, _buflen: i32, otype: i32) {
    let content = unsafe { CStr::from_ptr(buf) };
    let mutex = unsafe { KERNEL.as_ref().unwrap() };
    let mut kernel = mutex.lock().unwrap();
    kernel.write_console(content.to_str().unwrap(), otype);
}

pub fn start_r(
    iopub: SyncSender<IOPubMessage>,
    receiver: Receiver<RRequest>,
    initializer: Sender<RKernelInfo>,
) {
    use std::borrow::BorrowMut;

    let (console_send, console_recv) = channel::<Option<String>>();
    let (rprompt_send, rprompt_recv) = channel::<String>();
    let console = console_send.clone();

    // Initialize kernel (ensure we only do this once!)
    INIT.call_once(|| unsafe {
        *CONSOLE_RECV.borrow_mut() = Some(Mutex::new(console_recv));
        *RPROMPT_SEND.borrow_mut() = Some(Mutex::new(rprompt_send));
        let kernel = RKernel::new(iopub, console, initializer);
        *KERNEL.borrow_mut() = Some(Arc::new(Mutex::new(kernel)));
    });

    // Start thread to listen to execution requests
    thread::spawn(move || listen(receiver, rprompt_recv));

    // TODO: Discover R locations and populate R_HOME, a prerequisite to
    // initializing R.
    //
    // Maybe add a command line option to specify the path to R_HOME directly?
    unsafe {
        let arg1 = CString::new("ark").unwrap();
        let arg2 = CString::new("--interactive").unwrap();
        let mut args = vec![arg1.as_ptr(), arg2.as_ptr()];
        libR_sys::R_running_as_main_program = 1;
        libR_sys::R_SignalHandlers = 0;
        libR_sys::Rf_initialize_R(args.len() as i32, args.as_mut_ptr() as *mut *mut c_char);

        // Mark R session as interactive
        libR_sys::R_Interactive = 1;

        // Redirect console
        libR_sys::R_Consolefile = std::ptr::null_mut();
        libR_sys::R_Outputfile = std::ptr::null_mut();
        libR_sys::ptr_R_WriteConsole = None;
        libR_sys::ptr_R_WriteConsoleEx = Some(r_write_console);
        libR_sys::ptr_R_ReadConsole = Some(r_read_console);

        // Does not return
        trace!("Entering R main loop");
        libR_sys::Rf_mainloop();
        trace!("Exiting R main loop");
    }
}

fn handle_execute_request(req: &RRequest, prompt_recv: &Receiver<String>) {
    use extendr_api::prelude::*;

    // Service the execution request.
    let mutex = unsafe { KERNEL.as_ref().unwrap() };
    {
        let mut kernel = mutex.lock().unwrap();
        kernel.fulfill_request(&req)
    }

    // Wait for R to prompt us again. This signals that the
    // execution is finished and R is ready for input again.
    let default_prompt = R!(getOption("prompt")).unwrap().as_str().unwrap();
    trace!(
        "Waiting for R prompt signaling completion of execution (expected: '{}')",
        default_prompt
    );
    let prompt = prompt_recv.recv().unwrap();

    // Tell the kernel to complete the execution request.
    {
        let kernel = mutex.lock().unwrap();

        if prompt.starts_with("+") {
            // if the prompt is '+', we need to tell the kernel to emit an error
            trace!("Got R prompt '{}', marking request incomplete", prompt);
            kernel.report_incomplete_request(&req);
        } else if prompt != default_prompt {
            // if the prompt isn't the default, then it's likely a prompt from
            // R's `readline()` or similar; request input from the user.
            kernel.request_input(&prompt);
        } else {
            // for all other prompts, we can assume the request is complete
            trace!("Got R prompt '{}', finishing execution request", prompt);
            kernel.finish_request()
        }
    }
}

pub fn listen(exec_recv: Receiver<RRequest>, prompt_recv: Receiver<String>) {
    // Before accepting execution requests from the front end, wait for R to
    // prompt us for input.
    trace!("Waiting for R's initial input prompt...");
    let prompt = prompt_recv.recv().unwrap();
    trace!(
        "Got initial R prompt '{}', ready for execution requests",
        prompt
    );

    // Mark kernel as initialized as soon as we get the first input prompt from R
    let mutex = unsafe { KERNEL.as_ref().unwrap() };
    {
        let mut kernel = mutex.lock().unwrap();
        kernel.complete_intialization();
    }

    loop {
        // Wait for an execution request from the front end.
        match exec_recv.recv() {
            Ok(req) => handle_execute_request(&req, &prompt_recv),
            Err(err) => warn!("Could not receive execution request from kernel: {}", err),
        }
    }
}
