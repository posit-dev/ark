use std::ops::Deref;
use std::sync::LazyLock;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Sender;
use harp::RObject;
use libr::R_NilValue;
use libr::SEXP;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;

static RETICULATE_COMM_ID: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

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
pub unsafe extern "C" fn ps_reticulate_open(input: SEXP) -> Result<SEXP, anyhow::Error> {
    let input: RObject = input.try_into()?;
    let input_code: Option<String> = input.try_into()?;

    let mut comm_id_guard = RETICULATE_COMM_ID.lock().unwrap();

    // If there's an id already registered, we just need to send the focus event
    if let Some(id) = comm_id_guard.deref() {
        // There's a comm_id registered, we just send the focus event
        RMain::with(|main| {
            main.get_comm_manager_tx().send(CommManagerEvent::Message(
                id.clone(),
                CommMsg::Data(json!({
                    "method": "focus",
                    "params": {
                        "input": input_code
                    }
                })),
            ))
        })?;
        return Ok(R_NilValue);
    }

    let id = Uuid::new_v4().to_string();
    *comm_id_guard = Some(id.clone());

    RMain::with(|main| ReticulateService::start(id, main.get_comm_manager_tx().clone()))?;

    Ok(R_NilValue)
}
