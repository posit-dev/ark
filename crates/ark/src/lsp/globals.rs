//
// globals.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::event::CommEvent;
use crossbeam::channel::Sender;
use parking_lot::Mutex;
use parking_lot::MutexGuard;
use tower_lsp::Client;

use crate::request::KernelRequest;

// The LSP client.
// For use within R callback functions.
static mut LSP_CLIENT: Option<Client> = None;

// The shell request channel.
// For use within R callback functions.
static mut KERNEL_REQUEST_TX: Option<Mutex<Sender<KernelRequest>>> = None;

// The communication channel manager's request channel.
// For use within R callback functions.
static mut COMM_MANAGER_TX: Option<Mutex<Sender<CommEvent>>> = None;

pub(super) fn lsp_client() -> Client {
    unsafe { LSP_CLIENT.as_ref().unwrap_unchecked().clone() }
}

pub(super) fn kernel_request_tx<'a>() -> MutexGuard<'a, Sender<KernelRequest>> {
    unsafe { KERNEL_REQUEST_TX.as_ref().unwrap_unchecked().lock() }
}

pub(super) fn comm_manager_tx<'a>() -> MutexGuard<'a, Sender<CommEvent>> {
    unsafe { COMM_MANAGER_TX.as_ref().unwrap_unchecked().lock() }
}

pub fn initialize(
    lsp_client: Client,
    kernel_request_tx: Sender<KernelRequest>,
    comm_manager_tx: Sender<CommEvent>,
) {
    unsafe {
        LSP_CLIENT = Some(lsp_client);
        KERNEL_REQUEST_TX = Some(Mutex::new(kernel_request_tx));
        COMM_MANAGER_TX = Some(Mutex::new(comm_manager_tx));
    }
}
