//
// console_comm.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use std::rc::Rc;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommOutgoingTx;
use amalthea::socket::comm::CommSocket;
use stdext::result::ResultExt;
use stdext::DebugRefCell;
use uuid::Uuid;

use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::comm_handler::ConsoleComm;
use crate::comm_handler::EnvironmentChanged;
use crate::console::Console;
use crate::ui::UI_COMM_NAME;

// All methods take `&self`.
//
// Dispatch clones the comm's `Rc` out of `comms` via `lookup_comm()` and drops
// the map borrow before calling the handler. Two things fall out of that:
//
// 1. The map is free while the handler runs, so a handler that reenters the
//    map is fine. For instance, a data explorer handler calls
//    `comm_open_backend` to open a child explorer for a column, which
//    `borrow_mut()`s the map to insert.
//
// 2. Unlike a take approach that moves the comm out of the map and passes by
//    value to the handler, the comm stays in the map for the whole dispatch, and
//    is reachable by the Console the whole time. The UI comm relies on this: R
//    hooks send fire-and-forget events via `try_ui_comm()?.send_event().
impl Console {
    pub(super) fn comm_handle_msg(&self, comm_id: &str, msg: CommMsg) {
        let Some(comm) = self.lookup_comm(comm_id) else {
            log::warn!("Received message for unknown registered comm {comm_id}");
            return;
        };
        comm.handler.borrow_mut().handle_msg(msg, &comm.ctx);
        self.drain_closed();
    }

    pub(super) fn comm_handle_close(&self, comm_id: &str) {
        if let Some(comm) = self.remove_comm(comm_id) {
            comm.handler.borrow_mut().handle_close(&comm.ctx);
        }
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
        handler.handle_open(&ctx);

        self.comms.borrow_mut().insert(
            comm_id.clone(),
            Rc::new(ConsoleComm {
                handler: DebugRefCell::new(handler),
                ctx,
            }),
        );

        // Block until Shell has processed the open, ensuring the `comm_open`
        // message is on IOPub before we return. Any updates the caller sends
        // after this point are guaranteed to follow it.
        let (done_tx, done_rx) = crossbeam::channel::bounded(0);
        self.comm_event_tx
            .send(CommEvent::Opened(comm, open_metadata, Some(done_tx)))?;
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
        &self,
        comm_id: String,
        comm_name: &str,
        outgoing_tx: CommOutgoingTx,
        mut handler: Box<dyn CommHandler>,
    ) {
        let ctx = CommHandlerContext::new(outgoing_tx, self.comm_event_tx.clone());
        handler.handle_open(&ctx);

        if comm_name == UI_COMM_NAME {
            self.close_ui_comm();
            *self.ui_comm_id.borrow_mut() = Some(comm_id.clone());
        }

        self.comms.borrow_mut().insert(
            comm_id,
            Rc::new(ConsoleComm {
                handler: DebugRefCell::new(handler),
                ctx,
            }),
        );
    }

    pub(super) fn comm_notify_environment_changed(&self, event: &EnvironmentChanged) {
        // Snapshot the `Rc`s so the `comms` borrow is dropped before we run any
        // handler (a handler may reenter `self.comms`).
        let comms: Vec<Rc<ConsoleComm>> = self.comms.borrow().values().map(Rc::clone).collect();
        for comm in comms {
            comm.handler
                .borrow_mut()
                .handle_environment(event, &comm.ctx);
        }
        self.drain_closed();
    }

    // -- Comms map helpers ------------------------------------------------

    fn lookup_comm(&self, comm_id: &str) -> Option<Rc<ConsoleComm>> {
        self.comms.borrow().get(comm_id).map(Rc::clone)
    }

    pub(super) fn lookup_ui_comm(&self) -> Option<Rc<ConsoleComm>> {
        let comm_id = self.ui_comm_id.borrow();
        let comm_id = comm_id.as_deref()?;
        self.lookup_comm(comm_id)
    }

    /// Remove a comm from the map, keeping the UI index in sync.
    fn remove_comm(&self, comm_id: &str) -> Option<Rc<ConsoleComm>> {
        let comm = self.comms.borrow_mut().remove(comm_id)?;

        let mut ui_comm_id = self.ui_comm_id.borrow_mut();
        if ui_comm_id.as_deref() == Some(comm_id) {
            *ui_comm_id = None;
        }

        Some(comm)
    }

    /// Close and drop the currently registered UI comm, if any.
    fn close_ui_comm(&self) {
        let Some(old) = self
            .ui_comm_id
            .borrow_mut()
            .take()
            .and_then(|old_id| self.comms.borrow_mut().remove(&old_id))
        else {
            return;
        };
        log::info!("Replacing an existing UI comm.");
        old.handler.borrow_mut().handle_close(&old.ctx);
    }

    fn drain_closed(&self) {
        let closed_ids: Vec<String> = self
            .comms
            .borrow()
            .iter()
            .filter(|(_, comm)| comm.ctx.is_closed())
            .map(|(id, _)| id.clone())
            .collect();

        for comm_id in closed_ids {
            if let Some(comm) = self.remove_comm(&comm_id) {
                self.comm_notify_closed(&comm_id, &comm);
            }
        }
    }

    fn comm_notify_closed(&self, comm_id: &str, comm: &ConsoleComm) {
        comm.ctx.outgoing_tx.send(CommMsg::Close).log_err();
        comm.ctx
            .comm_event_tx
            .send(CommEvent::Closed(comm_id.to_string()))
            .log_err();
    }
}
