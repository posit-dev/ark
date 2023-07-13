//
// globals.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use crossbeam::channel::Sender;
use tower_lsp::Client;

use crate::request::KernelRequest;

// The global state used by R callbacks.
//
// Doesn't need a mutex because it's only accessed by the R thread. Should
// not be used elsewhere than from an R frontend callback or an R function
// invoked by the REPL.
pub(super) static mut R_CALLBACK_GLOBALS: Option<RCallbackGlobals> = None;

pub(super) struct RCallbackGlobals {
    pub(super) lsp_client: Client,
    pub(super) kernel_request_tx: Sender<KernelRequest>,
}

impl RCallbackGlobals {
    fn new(lsp_client: Client, kernel_request_tx: Sender<KernelRequest>) -> Self {
        Self {
            lsp_client,
            kernel_request_tx,
        }
    }
}

pub fn initialize(lsp_client: Client, kernel_request_tx: Sender<KernelRequest>) {
    unsafe {
        R_CALLBACK_GLOBALS = Some(RCallbackGlobals::new(lsp_client, kernel_request_tx));
    }
}
