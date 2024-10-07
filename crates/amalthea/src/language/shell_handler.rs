/*
 * shell_handler.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use async_trait::async_trait;

use crate::comm::comm_channel::Comm;
use crate::socket::comm::CommSocket;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::inspect_reply::InspectReply;
use crate::wire::inspect_request::InspectRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::originator::Originator;

#[async_trait]
pub trait ShellHandler: Send {
    /// Handles a request for information about the kernel.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#kernel-info
    async fn handle_info_request(
        &mut self,
        req: &KernelInfoRequest,
    ) -> crate::Result<KernelInfoReply>;

    /// Handles a request to test a fragment of code to see whether it is a
    /// complete expression.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#code-completeness
    async fn handle_is_complete_request(
        &self,
        req: &IsCompleteRequest,
    ) -> crate::Result<IsCompleteReply>;

    /// Handles a request to execute code.
    ///
    /// The `originator` is an opaque byte array identifying the peer that sent
    /// the request; it is needed to perform an input request during execution.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#execute
    async fn handle_execute_request(
        &mut self,
        originator: Originator,
        req: &ExecuteRequest,
    ) -> crate::Result<ExecuteReply>;

    /// Handles a request to provide completions for the given code fragment.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#completion
    async fn handle_complete_request(&self, req: &CompleteRequest) -> crate::Result<CompleteReply>;

    /// Handles a request to inspect a fragment of code.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#introspection
    async fn handle_inspect_request(&self, req: &InspectRequest) -> crate::Result<InspectReply>;

    /// Handles a request to open a comm.
    ///
    /// https://jupyter-client.readthedocs.io/en/stable/messaging.html#opening-a-comm
    ///
    /// Returns true if the handler handled the request (and opened the comm), false if it did not.
    ///
    /// * `target` - The target name of the comm, such as `positron.variables`
    /// * `comm` - The comm channel to use to communicate with the frontend
    async fn handle_comm_open(&self, target: Comm, comm: CommSocket) -> crate::Result<bool>;
}
