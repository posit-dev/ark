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

// These methods take `&mut self` and use `get_mut()` for zero-cost access to
// `self.comms`. The tradeoff: to pass `self` as `&Console` to handler methods,
// we must first take/remove the comm so no `&mut` borrow through `self` is
// active.
impl Console {
    pub(super) fn comm_handle_msg(&mut self, comm_id: &str, msg: CommMsg) {
        if self
            .ui_comm
            .as_ref()
            .is_some_and(|ui| ui.comm_id == comm_id)
        {
            let mut ui = self.ui_comm.take().unwrap();
            ui.handler.handle_msg(msg, &ui.ctx, self);
            self.ui_comm = Some(ui);
            return;
        }

        let Some(mut comm) = self.comms.get_mut().remove(comm_id) else {
            log::warn!("Received message for unknown registered comm {comm_id}");
            return;
        };
        comm.handler.handle_msg(msg, &comm.ctx, self);

        let key = comm.comm_id.clone();
        self.comms.get_mut().insert(key, comm);
        self.drain_closed();
    }

    pub(super) fn comm_handle_close(&mut self, comm_id: &str) {
        if self
            .ui_comm
            .as_ref()
            .is_some_and(|ui| ui.comm_id == comm_id)
        {
            let mut ui = self.ui_comm.take().unwrap();
            ui.handler.handle_close(&ui.ctx, self);
            return;
        }

        let Some(mut comm) = self.comm_remove(comm_id) else {
            log::warn!("Received close for unknown registered comm {comm_id}");
            return;
        };
        comm.handler.handle_close(&comm.ctx, self);
    }

    /// Register a backend-initiated comm on the R thread.
    ///
    /// Creates the `CommSocket` and `CommHandlerContext`, calls `handle_open`,
    /// sends `CommEvent::Opened` to Amalthea's Shell thread, and returns the
    /// comm ID.
    ///
    /// Blocks until Shell has fully processed the open (sent `comm_open` on
    /// IOPub and registered the comm for routing). This guarantees that any
    /// `comm_msg` sent by the caller afterwards are ordered after the
    /// `comm_open` on IOPub.
    pub(crate) fn comm_open_backend(
        &self,
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
        handler.handle_open(&ctx, self);

        self.comms
            .borrow_mut()
            .insert(comm_id.clone(), ConsoleComm {
                comm_id: comm_id.clone(),
                handler,
                ctx,
            });

        self.comm_event_tx
            .send(CommEvent::Opened(comm, open_metadata))?;

        // Block until Shell has processed the Opened event, ensuring the
        // `comm_open` message is on IOPub before we return. Any updates
        // the caller sends after this point are guaranteed to follow it.
        let (done_tx, done_rx) = crossbeam::channel::bounded(0);
        self.comm_event_tx.send(CommEvent::Barrier(done_tx))?;
        done_rx.recv()?;

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
        handler.handle_open(&ctx, self);

        if comm_name == UI_COMM_NAME {
            if let Some(mut old) = self.ui_comm.take() {
                log::info!("Replacing an existing UI comm.");
                old.handler.handle_close(&old.ctx, self);
            }
            self.ui_comm = Some(ConsoleComm {
                comm_id,
                handler,
                ctx,
            });
        } else {
            self.comms.get_mut().insert(comm_id.clone(), ConsoleComm {
                comm_id,
                handler,
                ctx,
            });
        }
    }

    pub(super) fn comm_notify_environment_changed(&mut self, event: &EnvironmentChanged) {
        if let Some(mut ui) = self.ui_comm.take() {
            ui.handler.handle_environment(event, &ui.ctx, self);
            self.ui_comm = Some(ui);
        }

        let ids: Vec<String> = self.comms.get_mut().keys().cloned().collect();
        for id in ids {
            let Some(mut comm) = self.comms.get_mut().remove(&id) else {
                continue;
            };
            comm.handler.handle_environment(event, &comm.ctx, self);
            self.comms.get_mut().insert(id, comm);
        }
        self.drain_closed();
    }

    /// Remove a comm from the map.
    fn comm_remove(&mut self, comm_id: &str) -> Option<ConsoleComm> {
        self.comms.get_mut().remove(comm_id)
    }

    /// Remove all comms whose handler requested closing via `ctx.close_on_exit()`.
    fn drain_closed(&mut self) {
        let closed_ids: Vec<String> = self
            .comms
            .get_mut()
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
