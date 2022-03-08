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
use serde_json::json;
use std::ffi::CString;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, Once};
use std::thread;

#[link(name = "R", kind = "dylib")]
extern "C" {
    /// Initialize R
    fn Rf_initialize_R(ac: c_int, av: *mut c_void) -> i32;

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

    /// Pointer to console write function
    static mut ptr_R_WriteConsole: *const c_void;

    static mut ptr_R_WriteConsoleEx: unsafe extern "C" fn(*mut c_char, i32, i32);
}

pub struct RKernel {
    pub execution_count: u32,
    iopub: Sender<IOPubMessage>,
}

static mut KERNEL: Option<Mutex<RKernel>> = None;
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
    prompt: *mut c_char,
    _buf: *mut c_char,
    _buflen: i32,
    _hist: i32,
) -> i32 {
    let r_prompt = unsafe { CString::from_raw(prompt) };
    let mutex = unsafe { KERNEL.as_ref().unwrap() };
    let kernel = mutex.lock().unwrap();
    kernel.read_console(r_prompt.into_string().unwrap());

    // Currently no input to read
    0
}

#[no_mangle]
pub extern "C" fn r_write_console(
    buf: *mut c_char,
    _buflen: i32,
    otype: i32
) {
    let content = unsafe { CString::from_raw(buf) };
    let mutex = unsafe { KERNEL.as_ref().unwrap() };
    let kernel = mutex.lock().unwrap();
    kernel.write_console(content.into_string().unwrap(), otype);
}

impl RKernel {
    pub fn start(iopub: Sender<IOPubMessage>, receiver: Receiver<ExecuteRequest>) {
        use std::borrow::BorrowMut;

        // Initialize kernel (ensure we only do this once!)
        INIT.call_once(|| unsafe {
            let kernel = Self {
                iopub: iopub,
                execution_count: 0,
            };
            *KERNEL.borrow_mut() = Some(Mutex::new(kernel));
        });

        // Start thread to listen to execution requests
        thread::spawn(move || Self::listen(receiver));

        // TODO: Discover R locations and populate R_HOME, a prerequisite to
        // initializing R.
        //
        // Maybe add a command line option to specify the path to R_HOME directly?
        unsafe {
            let arg1 = CString::new("ark").unwrap();
            let arg2 = CString::new("--interactive").unwrap();
            let args = Box::new(vec![arg1.as_ptr(), arg2.as_ptr()]);
            R_running_as_main_program = 1;
            R_Interactive = 1;
            R_Consolefile = std::ptr::null();
            R_Outputfile = std::ptr::null();

            // This must be set to NULL so that WriteConsoleEx is called
            ptr_R_WriteConsole = std::ptr::null();
            ptr_R_WriteConsoleEx = r_write_console;
            
            ptr_R_ReadConsole = r_read_console;
            Rf_initialize_R(args.len() as i32, Box::into_raw(args) as *mut c_void);

            // Does not return
            trace!("Entering R main loop");
            Rf_mainloop();
        }
    }

    pub fn listen(receiver: Receiver<ExecuteRequest>) {
        loop {
            match receiver.recv() {
                Ok(req) => {
                    // TODO: maybe this could be a with_kernel closure or something
                    let mutex = unsafe { KERNEL.as_ref().unwrap() };
                    let mut kernel = mutex.lock().unwrap();
                    kernel.execute_request(req)
                }
                Err(err) => warn!("Could not receive execution request from kernel: {}", err),
            }
        }
    }

    pub fn execute_request(&mut self, req: ExecuteRequest) {
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

    pub fn read_console(&self, prompt: String) {
        debug!("Read console from R with prompt: {}", prompt)
    }

    pub fn write_console(&self, content: String, otype: i32) {
        debug!("Write console {} from R: {}", otype, content);
        let data = json!({"text/plain": content });
        if let Err(err) = self.iopub.send(IOPubMessage::ExecuteResult(ExecuteResult {
            execution_count: self.execution_count,
            data: data,
            metadata: serde_json::Value::Null,
        })) {
            warn!(
                "Could not publish result of statement {} on iopub: {}",
                self.execution_count, err
            );
        }
    }
}
