/*
 * client.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::kernel::Kernel;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::jupyter_message::{Message, Status};
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::status::ExecutionState;
use env_logger;
use log::info;
use serde_json;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::thread;

mod control;
mod frontend;
mod shell;

// One-time construction of the logging system.
use std::sync::Once;
static INIT: Once = Once::new();
fn setup() {
    INIT.call_once(|| {
        env_logger::init();
    });
}

#[test]
fn test_kernel() {
    // Set up test logger
    setup();

    // This channel delivers execution status and other iopub messages from
    // other threads to the iopub thread
    let (iopub_sender, iopub_receiver) = sync_channel::<IOPubMessage>(10);

    let shell_sender = iopub_sender.clone();
    let shell = Arc::new(Mutex::new(shell::Shell::new(shell_sender)));
    let control = Arc::new(Mutex::new(control::Control {}));
    let frontend = frontend::Frontend::new();
    let connection_file = frontend.get_connection_file();

    // Create the thread that will run the Amalthea kernel
    thread::spawn(move || {
        let kernel = Kernel::new(connection_file).unwrap();
        kernel
            .connect(shell, control, iopub_sender, iopub_receiver)
            .unwrap();
    });

    // Ask the kernel for the kernel info. This should return an object with the
    // language "Test" defined in our shell handler.
    frontend.send_shell(KernelInfoRequest {});
    let reply = frontend.receive_shell();
    match reply {
        Message::KernelInfoReply(reply) => {
            info!("Kernel info: {:?}", reply);
            assert_eq!(reply.content.language_info.name, "Test");
        }
        _ => {
            panic!("Unexpected message received: {:?}", reply);
        }
    }

    // Ask the kernel to execute some code
    frontend.send_shell(ExecuteRequest {
        code: "42".to_string(),
        silent: false,
        store_history: true,
        user_expressions: serde_json::Value::Null,
        allow_stdin: false,
        stop_on_error: false,
    });

    // The kernel should send an execute reply message indicating that the execute succeeded
    let reply = frontend.receive_shell();
    match reply {
        Message::ExecuteReply(reply) => {
            assert_eq!(reply.content.status, Status::Ok);
        }
        _ => {
            panic!("Unexpected execute reply received: {:?}", reply);
        }
    }

    // The IOPub channel should receive four messages, in this order:
    // 1. A message indicating that the kernel has entered the busy state
    // 2. A message re-broadcasting the input
    // 3. A message with the result of the execution
    // 3. A message indicating that the kernel has exited the busy state

    // The first message should be an execution state message
    let iopub_1 = frontend.receive_iopub();
    match iopub_1 {
        Message::Status(status) => {
            assert_eq!(status.content.execution_state, ExecutionState::Busy);
        }
        _ => {
            panic!("Unexpected message received: {:?}", iopub_1);
        }
    }
}
