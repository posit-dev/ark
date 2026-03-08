//
// console_comm.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use stdext::result::ResultExt;
use uuid::Uuid;

use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::comm_handler::ConsoleComm;
use crate::comm_handler::EnvironmentChanged;
use crate::console::Console;

impl Console {
    pub(super) fn comm_handle_msg(&mut self, comm_id: &str, msg: CommMsg) {
        let Some(reg) = self.comms.get_mut(comm_id) else {
            log::warn!("Received message for unknown registered comm {comm_id}");
            return;
        };
        reg.handler.handle_msg(msg, &reg.ctx);
        self.drain_closed();
    }

    pub(super) fn comm_handle_close(&mut self, comm_id: &str) {
        let Some(mut reg) = self.comms.remove(comm_id) else {
            log::warn!("Received close for unknown registered comm {comm_id}");
            return;
        };
        reg.handler.handle_close(&reg.ctx);
    }

    /// Register a backend-initiated comm on the R thread.
    ///
    /// Creates the `CommSocket` and `CommHandlerContext`, calls `handle_open`,
    /// sends `CommEvent::Opened` to amalthea, and returns the comm ID.
    pub(crate) fn comm_register(
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

    pub(super) fn comm_notify_environment_changed(&mut self, event: EnvironmentChanged) {
        for (_, reg) in self.comms.iter_mut() {
            reg.handler.handle_environment(event, &reg.ctx);
        }
        self.drain_closed();
    }

    /// Remove all comms whose handler requested closing via `ctx.close_on_exit()`.
    fn drain_closed(&mut self) {
        let closed_ids: Vec<String> = self
            .comms
            .iter()
            .filter(|(_, reg)| reg.ctx.is_closed())
            .map(|(id, _)| id.clone())
            .collect();

        for comm_id in closed_ids {
            if let Some(reg) = self.comms.remove(&comm_id) {
                self.comm_notify_closed(&comm_id, &reg);
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
