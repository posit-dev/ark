/*
 * dummy_frontend.rs
 *
 * Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::collections::HashMap;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

use amalthea::comm::event::CommEvent;
use amalthea::fixtures::dummy_frontend::DummyConnection;
use amalthea::fixtures::dummy_frontend::DummyFrontend;
use amalthea::kernel;
use amalthea::kernel::StreamBehavior;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::socket::stdin::StdInRequest;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Sender;

use super::control;
use super::shell;

static AMALTHEA_FRONTEND: OnceLock<
    Arc<Mutex<(DummyFrontend, Sender<CommEvent>, Sender<IOPubMessage>)>>,
> = OnceLock::new();

/// Wrapper around `DummyFrontend` that checks sockets are empty on drop
pub struct DummyAmaltheaFrontend {
    pub comm_event_tx: Sender<CommEvent>,
    pub iopub_tx: Sender<IOPubMessage>,
    guard: MutexGuard<'static, (DummyFrontend, Sender<CommEvent>, Sender<IOPubMessage>)>,
}

impl DummyAmaltheaFrontend {
    pub fn lock() -> Self {
        let guard = Self::get_frontend().lock().unwrap();
        let comm_event_tx = guard.1.clone();
        let iopub_tx = guard.2.clone();
        Self {
            guard,
            comm_event_tx,
            iopub_tx,
        }
    }

    fn get_frontend(
    ) -> &'static Arc<Mutex<(DummyFrontend, Sender<CommEvent>, Sender<IOPubMessage>)>> {
        AMALTHEA_FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyAmaltheaFrontend::init())))
    }

    fn init() -> (DummyFrontend, Sender<CommEvent>, Sender<IOPubMessage>) {
        let connection = DummyConnection::new();
        let (connection_file, registration_file) = connection.get_connection_files();

        let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

        let (comm_event_tx, comm_event_rx) = bounded::<CommEvent>(10);

        let (stdin_request_tx, stdin_request_rx) = bounded::<StdInRequest>(1);
        let (stdin_reply_tx, stdin_reply_rx) = unbounded();

        let shell = Box::new(shell::Shell::new(
            iopub_tx.clone(),
            stdin_request_tx,
            stdin_reply_rx,
        ));
        let control = Arc::new(Mutex::new(control::Control {}));

        // Initialize logging
        env_logger::init();

        // Perform kernel connection on its own thread to
        // avoid deadlocking as it waits for the `HandshakeReply`
        stdext::spawn!("dummy_kernel", {
            let iopub_tx = iopub_tx.clone();
            move || {
                let server_handlers = HashMap::new();
                if let Err(err) = kernel::connect(
                    "amalthea",
                    connection_file,
                    Some(registration_file),
                    shell,
                    control,
                    server_handlers,
                    StreamBehavior::None,
                    iopub_tx,
                    iopub_rx,
                    comm_event_rx,
                    stdin_request_rx,
                    stdin_reply_tx,
                ) {
                    panic!("Error connecting kernel: {err:?}");
                };
            }
        });

        let frontend = DummyFrontend::from_connection(connection);
        (frontend, comm_event_tx, iopub_tx)
    }
}

// Check that we haven't left crumbs behind
impl Drop for DummyAmaltheaFrontend {
    fn drop(&mut self) {
        self.assert_no_incoming()
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyAmaltheaFrontend {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        &self.guard.0
    }
}

impl DerefMut for DummyAmaltheaFrontend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard.0
    }
}
