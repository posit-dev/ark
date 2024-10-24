use std::ops::Deref;
use std::sync::LazyLock;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::RObject;
use libr::R_NilValue;
use libr::SEXP;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;
use crate::modules::ARK_ENVS;

static RETICULATE_COMM_ID: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

pub struct ReticulateService {
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
}

impl ReticulateService {
    fn start(
        comm_id: String,
        comm_manager_tx: Sender<CommManagerEvent>,
        start_runtime: bool,
    ) -> anyhow::Result<String> {
        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            comm_id.clone(),
            String::from("positron.reticulate"),
        );

        let service = Self {
            comm,
            comm_manager_tx,
        };

        let event = CommManagerEvent::Opened(
            service.comm.clone(),
            json!({
                "start_runtime": start_runtime
            }),
        );

        service
            .comm_manager_tx
            .send(event)
            .or_log_error("Reticulate: Could not open comm.");

        spawn!(format!("ark-reticulate-{}", comm_id), move || {
            service
                .handle_messages()
                .or_log_error("Reticulate: Error handling messages");
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
                break;
            }

            // Forward data msgs to the frontend.
            // Functions don't have access to the comm object, so they send the data message
            // to the comm, which forwards it to the frontend.
            if let CommMsg::Data(_) = msg {
                self.comm.outgoing_tx.send(msg)?;
                continue;
            }
        }

        // before finalizing the thread we make sure to send a close message to the front end
        self.comm
            .outgoing_tx
            .send(CommMsg::Close)
            .or_log_error("Reticulate: Could not send close message to the front-end");

        // Reset the global comm_id before closing
        let mut comm_id_guard = RETICULATE_COMM_ID.lock().unwrap();
        log::info!("Reticulate Thread closing {:?}", (*comm_id_guard).clone());
        *comm_id_guard = None;

        Ok(())
    }
}

// Creates a client instance reticulate can use to communicate with the front-end.
// We should aim at having at most **1** client per R session.
// Further actions that reticulate can ask the front-end can be requested through
// the comm_id that is returned by this function.
#[harp::register]
pub unsafe extern "C" fn ps_reticulate_open(input: SEXP) -> anyhow::Result<SEXP> {
    // Get current comm_id
    let comm_id = RETICULATE_COMM_ID.lock().unwrap().deref().clone();

    // If there's already a comm_id, we just send a focus event with some code
    // to be executed.
    if let Some(id) = comm_id {
        let main = RMain::get();
        let input_code: Option<String> = RObject::from(input).try_into()?;

        // There's a comm_id registered, we just send the focus event
        main.get_comm_manager_tx().send(CommManagerEvent::Message(
            id.clone(),
            CommMsg::Data(json!({
                "method": "focus",
                "params": {
                    "input": input_code
                }
            })),
        ))?;
        return Ok(R_NilValue);
    } else {
        // We open a new comm_id, and start the reticulate runtime
        return Ok(ps_reticulate_open_comm(RObject::from(true).sexp));
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_reticulate_open_comm(start_runtime: SEXP) -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let mut comm_id_guard = RETICULATE_COMM_ID.lock().unwrap();

    // We only create a comm if there's no comm_id registered, otherwise this a no-op
    if let Some(_) = comm_id_guard.deref() {
        return Ok(R_NilValue);
    }

    let start: bool = RObject::from(start_runtime).try_into()?;
    let id = Uuid::new_v4().to_string();

    *comm_id_guard = Some(id.clone());
    ReticulateService::start(id, main.get_comm_manager_tx().clone(), start)?;

    // Register finalizer to cleanup the Python session when Reticulate is unloaded (or the
    // R session is about to end).
    RFunction::new("", "reticulate_register_finalizer").call_in(ARK_ENVS.positron_ns)?;

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_reticulate_shutdown() -> Result<SEXP, anyhow::Error> {
    log::info!("Reticulate: Shutdown called");
    let main = RMain::get();
    let comm_id = RETICULATE_COMM_ID.lock().unwrap().deref().clone();

    // Only send the shutdown message if there's a comm_id registered, otherwise
    // we don't need to do anything.
    if let Some(id) = comm_id {
        log::info!("Reticulate: Sending shutdown to {:?}", id);

        main.get_comm_manager_tx().send(CommManagerEvent::Message(
            id.clone(),
            CommMsg::Data(json!({
                "method": "shutdown",
            })),
        ))?;

        // Wait until the comm_id is deleted, which signals that the Reticulate Python
        // session has been successfully closed.
        let start = std::time::Instant::now();
        loop {
            if let None = RETICULATE_COMM_ID.lock().unwrap().deref().clone() {
                break;
            }

            if (std::time::Instant::now() - start) > std::time::Duration::from_secs(5) {
                log::warn!("Reticulate: Timeout waiting for reticulate to close");
                break;
            }

            log::info!("Waiting for reticulate to close");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        return Ok(R_NilValue);
    }

    Ok(R_NilValue)
}
