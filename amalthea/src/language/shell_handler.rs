/*
 * shell_handler.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::comm_info_reply::CommInfoReply;
use crate::wire::comm_info_request::CommInfoRequest;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::exception::Exception;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_reply_exception::ExecuteReplyException;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;

pub trait ShellHandler {
    fn handle_info_request(&self, req: KernelInfoRequest) -> Result<KernelInfoReply, Exception>;
    fn handle_is_complete_request(
        &self,
        req: IsCompleteRequest,
    ) -> Result<IsCompleteReply, Exception>;
    fn handle_execute_request(
        &self,
        req: ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException>;
    fn handle_complete_request(&self, req: CompleteRequest) -> Result<CompleteReply, Exception>;
    fn handle_comm_info_request(&self, req: CommInfoRequest) -> Result<CommInfoReply, Exception>;
}
