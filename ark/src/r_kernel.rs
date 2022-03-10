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
use libc::{c_char, c_int, c_void, strcpy };
use log::{debug, error, info, trace, warn};
use serde_json::json;
use std::ffi::{CString, CStr};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, Once};
use std::thread;
use std::sync::mpsc::channel;

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

    /// Signal handlers for R
    static mut R_SignalHandlers: c_int;

    // TODO: type of buffer isn't necessary c_char
    static mut ptr_R_ReadConsole: unsafe extern "C" fn(*mut c_char, *mut c_char, c_int, c_int) -> c_int;

    /// Pointer to console write function
    static mut ptr_R_WriteConsole: *const c_void;

    static mut ptr_R_WriteConsoleEx: unsafe extern "C" fn(*mut c_char, c_int, c_int);
}

pub struct RKernel {
    pub execution_count: u32,
    iopub: Sender<IOPubMessage>,
    console: Sender<String>,
    prompt: Receiver<String>,
    output: String
}

static mut KERNEL: Option<Mutex<RKernel>> = None;
static mut RPROMPT_SEND: Option<Mutex<Sender<String>>> = None;
static mut CONSOLE_RECV: Option<Mutex<Receiver<String>>> = None;
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
    buf: *mut c_char,
    buflen: c_int,
    _hist: c_int,
) -> i32 {
    let r_prompt = unsafe { CStr::from_ptr(prompt) };
    debug!("R prompt: {}", r_prompt.to_str().unwrap());
    
    // TODO: if R prompt is +, we need to tell the user their input is incomplete
    let mutex = unsafe { RPROMPT_SEND.as_ref().unwrap() };
    let sender = mutex.lock().unwrap();
    sender.send(r_prompt.to_string_lossy().into_owned()).unwrap();

    let mutex = unsafe { CONSOLE_RECV.as_ref().unwrap() };
    let recv = mutex.lock().unwrap();
    let mut input = recv.recv().unwrap();
    trace!("Sending input to R: '{}'", input);
    input.push_str("\n");
    if input.len() < buflen.try_into().unwrap() {
        let src = CString::new(input).unwrap();
        unsafe {
            libc::strcpy(buf, src.as_ptr());
        }
    } else {
        // Input doesn't fit in buffer
        // TODO: need to allow next call to read the buffer 
        return 1;
    }

    // Nonzero return values indicate the end of input and cause R to exit
    1
}

#[no_mangle]
pub extern "C" fn r_write_console(
    buf: *mut c_char,
    _buflen: i32,
    otype: i32
) {
    let content = unsafe { CStr::from_ptr(buf) };
    let mutex = unsafe { KERNEL.as_ref().unwrap() };
    let mut kernel = mutex.lock().unwrap();
    kernel.write_console(content.to_str().unwrap(), otype);
}

impl RKernel {
    pub fn start(iopub: Sender<IOPubMessage>, receiver: Receiver<ExecuteRequest>) {
        use std::borrow::BorrowMut;

        let (console_send, console_recv) = channel::<String>();
        let (rprompt_send, rprompt_recv) = channel::<String>();
        let console = console_send.clone();

        // Initialize kernel (ensure we only do this once!)
        INIT.call_once(|| unsafe {
            *CONSOLE_RECV.borrow_mut() = Some(Mutex::new(console_recv));
            *RPROMPT_SEND.borrow_mut() = Some(Mutex::new(rprompt_send));
            let kernel = Self {
                iopub: iopub,
                execution_count: 0,
                console: console,
                prompt: rprompt_recv,
                output: String::new()
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
            let mut args = vec![arg1.as_ptr(), arg2.as_ptr()];
            R_running_as_main_program = 1;
            R_SignalHandlers = 0;
            Rf_initialize_R(args.len() as i32, args.as_mut_ptr() as *mut c_void);

            // Mark R session as interactive
            R_Interactive = 1;

            // Redirect console
            R_Consolefile = std::ptr::null();
            R_Outputfile = std::ptr::null();
            ptr_R_WriteConsole = std::ptr::null();
            ptr_R_WriteConsoleEx = r_write_console;
            ptr_R_ReadConsole = r_read_console;

            // Does not return
            trace!("Entering R main loop");
            Rf_mainloop();
            trace!("Exiting R main loop");
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

        self.output = String::new();

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

        self.console.send(req.code).unwrap();
        let prompt = self.prompt.recv().unwrap();
        trace!("Completed execute request {} with R prompt: {} ... {}", self.execution_count, prompt, self.output);

        let data = json!({"text/plain": self.output });
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

    pub fn write_console(&mut self, content: &str, otype: i32) {
        debug!("Write console {} from R: {}", otype, content);
        self.output.push_str(content);
    }
}
