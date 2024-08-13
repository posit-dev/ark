use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use anyhow::anyhow;
use crossbeam::channel::Sender;
use harp::RObject;
use lazy_static::lazy_static;
use libr::R_NilValue;
use libr::SEXP;
use serde_json::json;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;

lazy_static! {
    static ref RETICULATE_COMM_ID: Mutex<Option<String>> = Mutex::new(None);
}

pub struct ReticulateService {
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
}

impl ReticulateService {
    fn start(comm_id: String, comm_manager_tx: Sender<CommManagerEvent>) -> anyhow::Result<String> {
        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            comm_id.clone(),
            String::from("positron.reticulate"),
        );

        let service = Self {
            comm,
            comm_manager_tx,
        };

        let event = CommManagerEvent::Opened(service.comm.clone(), serde_json::Value::Null);
        unwrap!(service.comm_manager_tx.send(event), Err(e) => {
            log::error!("Reticulate: Could not open comm. Error {e}");
        });

        spawn!(format!("ark-reticulate-{}", comm_id), move || {
            unwrap!(service.handle_messages(), Err(err) => {
                log::error!("Connection Pane: Error while handling messages: {err:?}");
            });
        });

        Ok(comm_id)
    }

    fn handle_messages(&self) -> Result<(), anyhow::Error> {
        loop {
            let msg = unwrap!(self.comm.incoming_rx.recv(), Err(err) => {
                log::error!("Reticulate: Error while receiving message from frontend: {err:?}");
                break;
            });

            if let CommMsg::Close = msg {
                self.comm.outgoing_tx.send(CommMsg::Close).unwrap();
                break;
            }

            // Forward data msgs to the frontend
            if let CommMsg::Data(_) = msg {
                self.comm.outgoing_tx.send(msg)?;
                continue;
            }
        }

        // before finalizing the thread we make sure to send a close message to the front end
        if let Err(err) = self.comm.outgoing_tx.send(CommMsg::Close) {
            log::error!("Reticulate: Error while sending comm_close to front end: {err:?}");
        }

        let mut comm_id_guard = unwrap!(
            RETICULATE_COMM_ID.try_lock(),
            Err(e) => {
                return Err(anyhow!("Could not access comm_id. Error {}", e));
            }
        );
        log::info!(
            "Reticulate Thread closing {}",
            (*comm_id_guard).clone().unwrap()
        );

        *comm_id_guard = None;

        Ok(())
    }
}

// Creates a client instance reticulate can use to communicate with the front-end.
// We should aim at having at most **1** client per R session.
// Further actions that reticulate can ask the front-end can be requested through
// the comm_id that is returned by this function.
#[harp::register]
pub unsafe extern "C" fn ps_reticulate_open() -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();

    if !RMain::initialized() {
        return Ok(R_NilValue);
    }

    // If there's an id already registered, we just need to send the focus event
    let mut comm_id_guard = unwrap!(
        RETICULATE_COMM_ID.try_lock(),
        Err(e) => {
            return Err(anyhow!("Could not access comm_id. Error {}", e));
        }
    );

    if let Some(id) = (*comm_id_guard).clone() {
        // There's a comm_id registered, we just send the focus event
        main.get_comm_manager_tx().send(CommManagerEvent::Message(
            id,
            CommMsg::Data(json!({
                "method": "focus",
                "params": {}
            })),
        ))?;
        return Ok(R_NilValue);
    }

    let id = Uuid::new_v4().to_string();
    *comm_id_guard = Some(id.clone());
    unwrap! (
        ReticulateService::start(id, main.get_comm_manager_tx().clone()),
        Err(err) => {
            log::error!("Reticulate: Failed to start connection: {err:?}");
            return Err(err);
        }
    );

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_reticulate_focus(id: SEXP) -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();
    let comm_id: String = RObject::view(id).to::<String>()?;

    main.get_comm_manager_tx().send(CommManagerEvent::Message(
        comm_id,
        CommMsg::Data(json!({
            "method": "focus",
            "params": {}
        })),
    ))?;

    Ok(R_NilValue)
}
