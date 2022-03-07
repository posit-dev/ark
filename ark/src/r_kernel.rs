/*
 * r_kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_result::ExecuteResult;
use libc::{c_char, c_int, c_void};
use log::{debug, error, info, trace, warn};
use std::ffi::CString;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

#[link(name = "R", kind = "dylib")]
extern "C" {
    // TODO: the arg array doesn't seem to be passable in a type safe way, should just be a raw pointer
    fn Rf_initialize_R(ac: c_int, av: &[*const c_char]) -> i32;

    /// Run the R main execution loop (does not return)
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

pub struct RKernel {
    execution_count: u32,
}

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
    pub fn start(sender: Sender<IOPubMessage>, receiver: Receiver<ExecuteRequest>) {
        // Start thread to listen to execution requests
        thread::spawn(move || Self::listen(sender, receiver));

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

    pub fn listen(sender: Sender<IOPubMessage>, receiver: Receiver<ExecuteRequest>) {
        loop {
            // TODO: should lock executor?
            match receiver.recv() {
                Ok(req) => Self::execute_request(sender, req),
                Err(err) => warn!("Could not receive execution request from kernel: {}", err),
            }
        }
    }

    pub fn execute_request(sender: Sender<IOPubMessage>, req: ExecuteRequest) {
        // Increment counter if we are storing this execution in history
        if req.store_history {
            self.execution_count = self.execution_count + 1;
        }

        // If the code is not to be executed silently, re-broadcast the
        // execution to all frontends
        if !req.silent {
            if let Err(err) = self.iopub.send(IOPubMessage::ExecuteInput(ExecuteInput {
                code: req.code.clone(),
                execution_count: self.execution_count,
            })) {
                warn!(
                    "Could not broadcast execution input {} to all front ends: {}",
                    self.execution_count, err
                );
            }
        }

        // For this toy echo language, generate a result that's just the input
        // echoed back.
        let data = json!({"text/plain": req.code });
        if let Err(err) = self.iopub.send(IOPubMessage::ExecuteResult(ExecuteResult {
            execution_count: self.execution_count,
            data: data,
            metadata: serde_json::Value::Null,
        })) {
            warn!(
                "Could not publish result of computation {} on iopub: {}",
                self.execution_count, err
            );
        }
    }
}
