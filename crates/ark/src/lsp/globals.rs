//
// globals.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::Client;

// The global state used by R callbacks.
//
// Doesn't need a mutex because it's only accessed by the R thread. Should
// not be used elsewhere than from an R frontend callback or an R function
// invoked by the REPL.
pub(super) static mut R_CALLBACK_GLOBALS: Option<RCallbackGlobals> = None;

pub(super) struct RCallbackGlobals {
    pub(super) lsp_client: Client,
}

impl RCallbackGlobals {
    fn new(lsp_client: Client) -> Self {
        Self { lsp_client }
    }
}

pub fn initialize(lsp_client: Client) {
    unsafe {
        R_CALLBACK_GLOBALS = Some(RCallbackGlobals::new(lsp_client));
    }
}
