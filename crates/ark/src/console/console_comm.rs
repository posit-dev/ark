//
// console_comm.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommOutgoingTx;
use amalthea::socket::comm::CommSocket;
use stdext::result::ResultExt;
use uuid::Uuid;

use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::comm_handler::ConsoleComm;
use crate::comm_handler::EnvironmentChanged;
use crate::console::Console;
use crate::ui::UI_COMM_NAME;

impl Console {
    pub(super) fn comm_handle_msg(&mut self, comm_id: &str, msg: CommMsg) {
        let Some(comm) = self.comms.get_mut(comm_id) else {
            log::warn!("Received message for unknown registered comm {comm_id}");
            return;
        };
        comm.handler.handle_msg(msg, &comm.ctx);
        self.drain_closed();
    }

    pub(super) fn comm_handle_close(&mut self, comm_id: &str) {
        let Some(mut comm) = self.comm_remove(comm_id) else {
            log::warn!("Received close for unknown registered comm {comm_id}");
            return;
        };
        comm.handler.handle_close(&comm.ctx);
    }

    /// Register a backend-initiated comm on the R thread.
    ///
    /// Creates the `CommSocket` and `CommHandlerContext`, calls `handle_open`,
    /// sends `CommEvent::Opened` to amalthea, and returns the comm ID.
    pub(crate) fn comm_open_backend(
        &mut self,
        comm_name: &str,
        mut handler: Box<dyn CommHandler>,
    ) -> anyhow::Result<String> {
        let open_metadata = handler.open_metadata();
        let comm_id = Uuid::new_v4().to_string();

        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            comm_id.clone(),
            String::from(comm_name),
            self.iopub_tx().clone(),
        );

        let ctx = CommHandlerContext::new(comm.outgoing_tx.clone(), self.comm_event_tx.clone());
        handler.handle_open(&ctx);

        self.comms
            .insert(comm_id.clone(), ConsoleComm { handler, ctx });

        self.comm_event_tx
            .send(CommEvent::Opened(comm, open_metadata))?;

        Ok(comm_id)
    }

    /// Register a frontend-initiated comm on the R thread.
    ///
    /// Unlike `comm_open_backend` (which is for backend-initiated comms and
    /// sends `CommEvent::Opened`), this is called when the frontend opened the
    /// comm. The `CommSocket` already exists in amalthea's open_comms list, so
    /// we only need to register the handler and call `handle_open`.
    pub(super) fn comm_open_frontend(
        &mut self,
        comm_id: String,
        comm_name: &str,
        outgoing_tx: CommOutgoingTx,
        mut handler: Box<dyn CommHandler>,
    ) {
        let ctx = CommHandlerContext::new(outgoing_tx, self.comm_event_tx.clone());
        handler.handle_open(&ctx);

        if comm_name == UI_COMM_NAME {
            if let Some(old_id) = self.ui_comm_id.take() {
                log::info!("Replacing an existing UI comm.");
                if let Some(mut old) = self.comm_remove(&old_id) {
                    old.handler.handle_close(&old.ctx);
                }
            }
            self.ui_comm_id = Some(comm_id.clone());
        }

        self.comms.insert(comm_id, ConsoleComm { handler, ctx });
    }

    pub(super) fn comm_notify_environment_changed(&mut self, event: &EnvironmentChanged) {
        for (_, comm) in self.comms.iter_mut() {
            comm.handler.handle_environment(event, &comm.ctx);
        }
        self.drain_closed();
    }

    /// Remove a comm from the map, clearing `ui_comm_id` if it matches.
    fn comm_remove(&mut self, comm_id: &str) -> Option<ConsoleComm> {
        if self.ui_comm_id.as_deref() == Some(comm_id) {
            self.ui_comm_id = None;
        }
        self.comms.remove(comm_id)
    }

    /// Remove all comms whose handler requested closing via `ctx.close_on_exit()`.
    fn drain_closed(&mut self) {
        let closed_ids: Vec<String> = self
            .comms
            .iter()
            .filter(|(_, comm)| comm.ctx.is_closed())
            .map(|(id, _)| id.clone())
            .collect();

        for comm_id in closed_ids {
            if let Some(comm) = self.comm_remove(&comm_id) {
                self.comm_notify_closed(&comm_id, &comm);
            }
        }
    }

    /// Backend-initiated close cleanup: notify frontend via amalthea.
    fn comm_notify_closed(&self, comm_id: &str, comm: &ConsoleComm) {
        comm.ctx.outgoing_tx.send(CommMsg::Close).log_err();
        comm.ctx
            .comm_event_tx
            .send(CommEvent::Closed(comm_id.to_string()))
            .log_err();
    }
}
