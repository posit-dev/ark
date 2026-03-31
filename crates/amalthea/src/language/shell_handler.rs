/*
 * shell_handler.rs
 *
 * Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
 *
 */

use async_trait::async_trait;

use crate::comm::comm_channel::Comm;
use crate::comm::comm_channel::CommMsg;
use crate::socket::comm::CommSocket;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::history_reply::HistoryReply;
use crate::wire::history_request::HistoryRequest;
use crate::wire::inspect_reply::InspectReply;
use crate::wire::inspect_request::InspectRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::originator::Originator;

/// Result of a `handle_comm_msg` or `handle_comm_close` call on the
/// `ShellHandler`. `Handled` means the kernel processed the message
/// synchronously (blocking Shell until done). `NotHandled` means amalthea
/// should fall back to the historical `incoming_tx` path. This fallback is
/// temporary until all comms are migrated to the blocking path.
pub enum CommHandled {
    Handled,
    NotHandled,
}

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

    /// Handles a request for execution history.
    ///
    /// Docs: https://jupyter-client.readthedocs.io/en/stable/messaging.html#history
    async fn handle_history_request(&self, req: &HistoryRequest) -> crate::Result<HistoryReply>;

    /// Handles a request to open a comm.
    ///
    /// https://jupyter-client.readthedocs.io/en/stable/messaging.html#opening-a-comm
    ///
    /// Returns true if the handler handled the request (and opened the comm), false if it did not.
    ///
    /// * `target` - The target name of the comm, such as `positron.variables`
    /// * `comm` - The comm channel to use to communicate with the frontend
    /// * `data` - The `data` payload from the `comm_open` message
    async fn handle_comm_open(
        &self,
        target: Comm,
        comm: CommSocket,
        data: serde_json::Value,
    ) -> crate::Result<bool>;

    /// Handle an incoming comm message (RPC or data). Return
    /// `CommHandled::Handled` if the message was processed, or
    /// `CommHandled::NotHandled` to fall back to the existing
    /// `incoming_tx` path.
    ///
    /// * `comm_id` - The comm's unique identifier
    /// * `comm_name` - The comm's target name (e.g. `"positron.dataExplorer"`)
    /// * `msg` - The parsed `CommMsg`
    /// * `originator` - The originator of the Jupyter message, threaded through
    ///   so that comm handlers can make RPCs back to the frontend
    fn handle_comm_msg(
        &mut self,
        _comm_id: &str,
        _comm_name: &str,
        _msg: CommMsg,
        _originator: Originator,
    ) -> crate::Result<CommHandled> {
        Ok(CommHandled::NotHandled)
    }

    /// Handle a comm close. Return `CommHandled::Handled` if the close
    /// was processed, or `CommHandled::NotHandled` to fall back to the
    /// existing `incoming_tx` path.
    ///
    /// * `comm_id` - The comm's unique identifier
    /// * `comm_name` - The comm's target name
    fn handle_comm_close(
        &mut self,
        _comm_id: &str,
        _comm_name: &str,
    ) -> crate::Result<CommHandled> {
        Ok(CommHandled::NotHandled)
    }
}
