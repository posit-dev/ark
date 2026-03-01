//
// console_comm.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommEvent;
use crossbeam::channel::Sender;
use stdext::result::ResultExt;

use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::comm_handler::RegisteredComm;
use crate::console::Console;

impl Console {
    pub(crate) fn comm_handle_open(
        &mut self,
        comm_id: String,
        comm_name: String,
        mut handler: Box<dyn CommHandler>,
        ctx: CommHandlerContext,
    ) {
        handler.handle_open(&ctx);
        self.comms.insert(comm_id, RegisteredComm {
            handler,
            ctx,
            comm_name,
        });
    }

    pub(crate) fn comm_handle_msg(&mut self, comm_id: &str, msg: CommMsg) {
        let Some(reg) = self.comms.get_mut(comm_id) else {
            log::warn!("Received message for unknown registered comm {comm_id}");
            return;
        };
        reg.handler.handle_msg(msg, &reg.ctx);
        if reg.ctx.is_closed() {
            let reg = self.comms.remove(comm_id).unwrap();
            self.comm_notify_closed(comm_id, &reg);
        }
    }

    pub(crate) fn comm_handle_close(&mut self, comm_id: &str) {
        let Some(mut reg) = self.comms.remove(comm_id) else {
            log::warn!("Received close for unknown registered comm {comm_id}");
            return;
        };
        reg.handler.handle_close(&reg.ctx);
    }

    /// Register a comm handler directly from the R thread (backend-initiated comms).
    /// Calls `handle_open` immediately since we're already on the R thread.
    pub fn comm_register(
        &mut self,
        comm_id: String,
        comm_name: String,
        mut handler: Box<dyn CommHandler>,
        ctx: CommHandlerContext,
    ) {
        handler.handle_open(&ctx);
        self.comms.insert(comm_id, RegisteredComm {
            handler,
            ctx,
            comm_name,
        });
    }

    pub(crate) fn comm_notify_environment_changed(&mut self) {
        let mut closed_ids = Vec::new();
        for (comm_id, reg) in self.comms.iter_mut() {
            reg.handler.handle_environment(&reg.ctx);
            if reg.ctx.is_closed() {
                closed_ids.push(comm_id.clone());
            }
        }
        for comm_id in closed_ids {
            let reg = self.comms.remove(&comm_id).unwrap();
            self.comm_notify_closed(&comm_id, &reg);
        }
    }

    pub fn comm_event_tx(&self) -> &Sender<CommEvent> {
        &self.comm_event_tx
    }

    /// Backend-initiated close cleanup: notify frontend and amalthea.
    fn comm_notify_closed(&self, comm_id: &str, reg: &RegisteredComm) {
        reg.ctx.outgoing_tx.send(CommMsg::Close).log_err();
        self.comm_event_tx
            .send(CommEvent::Closed(comm_id.to_string()))
            .log_err();
    }
}
