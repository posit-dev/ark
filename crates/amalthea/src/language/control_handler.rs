/*
 * control_handler.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use crate::wire::debug_reply::DebugReply;
use crate::wire::debug_request::DebugRequest;
use crate::wire::exception::Exception;
use crate::wire::interrupt_reply::InterruptReply;
use crate::wire::shutdown_reply::ShutdownReply;
use crate::wire::shutdown_request::ShutdownRequest;

pub trait ControlHandler: Send {
    /// Handles a request to shut down the kernel. This message is forwarded
    /// from the Control socket.
    ///
    /// https://jupyter-client.readthedocs.io/en/stable/messaging.html#kernel-shutdown
    fn handle_shutdown_request(&self, msg: &ShutdownRequest) -> Result<ShutdownReply, Exception>;

    /// Handles a request to interrupt the kernel. This message is forwarded
    /// from the Control socket.
    ///
    /// https://jupyter-client.readthedocs.io/en/stable/messaging.html#kernel-interrupt
    fn handle_interrupt_request(&self) -> Result<InterruptReply, Exception>;

    /// Handles a debug request forwarded from the Control socket.
    /// The request and reply contents are opaque DAP messages.
    ///
    /// https://jupyter-client.readthedocs.io/en/latest/messaging.html#debug-request
    fn handle_debug_request(&self, msg: &DebugRequest) -> Result<DebugReply, Exception>;
}
