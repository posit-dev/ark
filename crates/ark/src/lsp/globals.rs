//
// globals.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tokio::runtime::Runtime;
use tower_lsp::Client;

// The global state used by R callbacks.
//
// Doesn't need a mutex because it's only accessed by the R thread. Should
// not be used elsewhere than from an R frontend callback or an R function
// invoked by the REPL.
pub(super) static mut R_CALLBACK_GLOBALS: Option<RCallbackGlobals> = None;
pub(super) static mut R_CALLBACK_GLOBALS2: Option<RCallbackGlobals2> = None;

pub(super) struct RCallbackGlobals {
    pub(super) lsp_client: Client,
}
pub(super) struct RCallbackGlobals2 {
    pub(super) lsp_runtime: Runtime,
}

impl RCallbackGlobals {
    fn new(lsp_client: Client) -> Self {
        Self { lsp_client }
    }
}

impl RCallbackGlobals2 {
    fn new(lsp_runtime: Runtime) -> Self {
        Self { lsp_runtime }
    }
}

pub fn initialize(lsp_client: Client) {
    unsafe {
        R_CALLBACK_GLOBALS = Some(RCallbackGlobals::new(lsp_client));
    }
}

pub fn initialize2(lsp_runtime: Runtime) {
    unsafe {
        R_CALLBACK_GLOBALS2 = Some(RCallbackGlobals2::new(lsp_runtime));
    }
}
