//
// console_comm.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use std::cell::RefCell;

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

// All methods take `&self`.
//
// Regular comms use a take/remove pattern: we take the comm out of the
// `comms` HashMap before calling the handler, so no `borrow_mut()` guard
// is held during the call. This prevents panics if the handler reenters
// the HashMap. For instance, a data explorer handler calls
// `comm_open_backend` to open a child explorer for a column, which
// needs to `borrow_mut()` the same HashMap to insert.
//
// The UI comm uses a different strategy: the handler is in its own
// `RefCell` inside the `ConsoleComm`, and we borrow the outer
// `ui_comm: RefCell<Option<ConsoleComm>>` with a shared `&` ref during
// dispatch. This keeps the `CommHandlerContext` (and thus the outgoing channel)
// visible to reentrant code that calls `ui_comm()`, e.g. R hooks that send
// fire-and-forget events via `try_ui_comm()?.send_event()`.
impl Console {
    pub(super) fn comm_handle_msg(&self, comm_id: &str, msg: CommMsg) {
        if self.is_ui_comm(comm_id) {
            self.with_ui_handler_mut(|handler, ctx| {
                handler.handle_msg(msg, ctx);
            });
            return;
        }

        self.with_comm_mut(comm_id, |comm| {
            comm.handler.get_mut().handle_msg(msg, &comm.ctx);
        });
        self.drain_closed();
    }

    pub(super) fn comm_handle_close(&self, comm_id: &str) {
        if self.is_ui_comm(comm_id) {
            let ui = self.take_ui_comm().unwrap();
            ui.handler.into_inner().handle_close(&ui.ctx);
            return;
        }

        if let Some(comm) = self.take_comm(comm_id) {
            comm.handler.into_inner().handle_close(&comm.ctx);
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

        self.comms
            .borrow_mut()
            .insert(comm_id.clone(), ConsoleComm {
                comm_id: comm_id.clone(),
                handler: RefCell::new(handler),
                ctx,
            });

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
            if let Some(old) = self.take_ui_comm() {
                log::info!("Replacing an existing UI comm.");
                old.handler.into_inner().handle_close(&old.ctx);
            }
            self.set_ui_comm(ConsoleComm {
                comm_id,
                handler: RefCell::new(handler),
                ctx,
            });
        } else {
            self.comms
                .borrow_mut()
                .insert(comm_id.clone(), ConsoleComm {
                    comm_id,
                    handler: RefCell::new(handler),
                    ctx,
                });
        }
    }

    pub(super) fn comm_notify_environment_changed(&self, event: &EnvironmentChanged) {
        self.with_ui_handler_mut(|handler, ctx| {
            handler.handle_environment(event, ctx);
        });

        let ids: Vec<String> = self.comms.borrow().keys().cloned().collect();
        for id in ids {
            self.with_comm_mut(&id, |comm| {
                comm.handler.get_mut().handle_environment(event, &comm.ctx);
            });
        }
        self.drain_closed();
    }

    // -- UI comm helpers --------------------------------------------------

    fn is_ui_comm(&self, comm_id: &str) -> bool {
        self.ui_comm
            .borrow()
            .as_ref()
            .is_some_and(|ui| ui.comm_id == comm_id)
    }

    /// Borrow the UI comm with `&`, then borrow the handler with `&mut`.
    ///
    /// Because the outer `RefCell` is only borrowed by shared ref, `ui_comm()`
    /// remains functional during handler dispatch and R code that calls
    /// back into Rust (e.g. `navigateToFile` from a `frontend_ready`
    /// hook) can still send events on the UI comm.
    fn with_ui_handler_mut(&self, f: impl FnOnce(&mut Box<dyn CommHandler>, &CommHandlerContext)) {
        let guard = self.ui_comm.borrow();
        let Some(ui) = guard.as_ref() else {
            log::warn!("UI comm is absent during dispatch (reentrant call?)");
            return;
        };
        let mut handler = ui.handler.borrow_mut();
        f(&mut handler, &ui.ctx);
    }

    fn take_ui_comm(&self) -> Option<ConsoleComm> {
        self.ui_comm.borrow_mut().take()
    }

    fn set_ui_comm(&self, ui: ConsoleComm) {
        *self.ui_comm.borrow_mut() = Some(ui);
    }

    // -- Comms map helpers ------------------------------------------------

    /// Take a comm out, call `f`, put it back.
    fn with_comm_mut(&self, comm_id: &str, f: impl FnOnce(&mut ConsoleComm)) {
        let Some(mut comm) = self.take_comm(comm_id) else {
            log::warn!("Received message for unknown registered comm {comm_id}");
            return;
        };
        f(&mut comm);
        self.comms.borrow_mut().insert(comm.comm_id.clone(), comm);
    }

    fn take_comm(&self, comm_id: &str) -> Option<ConsoleComm> {
        self.comms.borrow_mut().remove(comm_id)
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
            if let Some(comm) = self.take_comm(&comm_id) {
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
