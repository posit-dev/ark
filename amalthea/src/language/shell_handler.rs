/*
 * shell_handler.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::comm_info_reply::CommInfoReply;
use crate::wire::comm_info_request::CommInfoRequest;
use crate::wire::comm_msg::CommMsg;
use crate::wire::comm_open::CommOpen;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::exception::Exception;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_reply_exception::ExecuteReplyException;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::input_reply::InputReply;
use crate::wire::input_request::ShellInputRequest;
use crate::wire::inspect_reply::InspectReply;
use crate::wire::inspect_request::InspectRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;

use std::sync::mpsc::SyncSender;

use async_trait::async_trait;

#[async_trait]
pub trait ShellHandler: Send {
    /// Handles a request for information about the kernel.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#kernel-info
    async fn handle_info_request(
        &mut self,
        req: &KernelInfoRequest,
    ) -> Result<KernelInfoReply, Exception>;

    /// Handles a request to test a fragment of code to see whether it is a
    /// complete expression.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#code-completeness
    async fn handle_is_complete_request(
        &self,
        req: &IsCompleteRequest,
    ) -> Result<IsCompleteReply, Exception>;

    /// Handles a request to execute code.
    ///
    /// The `originator` is an opaque byte array identifying the peer that sent
    /// the request; it is needed to perform an input request during execution.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#execute
    async fn handle_execute_request(
        &mut self,
        originator: &Vec<u8>,
        req: &ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException>;

    /// Handles a request to provide completions for the given code fragment.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#completion
    async fn handle_complete_request(
        &self,
        req: &CompleteRequest,
    ) -> Result<CompleteReply, Exception>;

    /// Handles a request to return info on open comms.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#comm-info
    async fn handle_comm_info_request(
        &self,
        req: &CommInfoRequest,
    ) -> Result<CommInfoReply, Exception>;

    /// Handles a request to inspect a fragment of code.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#introspection
    async fn handle_inspect_request(&self, req: &InspectRequest)
        -> Result<InspectReply, Exception>;

    /// Handles a request to send a message to a comm.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#comm-messages
    async fn handle_comm_msg(&self, msg: &CommMsg) -> Result<(), Exception>;

    /// Handles a request to open a comm.
    ///
    /// https://jupyter-client.readthedocs.io/en/stable/messaging.html#opening-a-comm
    async fn handle_comm_open(&self, msg: &CommOpen) -> Result<(), Exception>;

    /// Handles a reply to a request for input from the front end (from stdin socket)
    ///
    /// https://jupyter-client.readthedocs.io/en/stable/messaging.html#messages-on-the-stdin-router-dealer-channel
    async fn handle_input_reply(&self, msg: &InputReply) -> Result<(), Exception>;

    /// Establishes an input handler for the front end (from stdin socket); when
    /// input is needed, the language runtime can request it by sending an
    /// InputRequest to this channel. The front end will prompt the user for
    /// input and deliver it via the `handle_input_reply` method.
    ///
    /// https://jupyter-client.readthedocs.io/en/stable/messaging.html#messages-on-the-stdin-router-dealer-channel
    fn establish_input_handler(&mut self, handler: SyncSender<ShellInputRequest>);
}
