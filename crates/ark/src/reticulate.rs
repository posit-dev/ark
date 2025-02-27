use std::ops::Deref;
use std::sync::LazyLock;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Sender;
use harp::utils::r_is_null;
use harp::RObject;
use libr::R_NilValue;
use libr::SEXP;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;

static RETICULATE_OUTGOING_TX: LazyLock<Mutex<Option<Sender<CommMsg>>>> =
    LazyLock::new(|| Mutex::new(None));

#[derive(Clone)]
pub struct ReticulateService {
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
}

impl ReticulateService {
    fn start(
        comm_id: String,
        comm_manager_tx: Sender<CommManagerEvent>,
        input: Option<String>,
    ) -> anyhow::Result<()> {
        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            comm_id.clone(),
            String::from("positron.reticulate"),
        );

        {
            let mut outgoing_tx_guard = RETICULATE_OUTGOING_TX.lock().unwrap();
            *outgoing_tx_guard = Some(comm.outgoing_tx.clone());
        }

        let service = Self {
            comm,
            comm_manager_tx,
        };

        let event = CommManagerEvent::Opened(
            service.comm.clone(),
            json!({
                "input": input
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

        Ok(())
    }

    fn handle_messages(&self) -> Result<(), anyhow::Error> {
        loop {
            let msg: CommMsg = unwrap!(self.comm.incoming_rx.recv(), Err(err) => {
                log::error!("Reticulate: Error while receiving message from frontend: {err:?}");
                break;
            });

            log::trace!("Reticulate: Received message from front end: {msg:?}");

            if let CommMsg::Close = msg {
                break;
            }
        }

        // before finalizing the thread we make sure to send a close message to the front end
        self.comm
            .outgoing_tx
            .send(CommMsg::Close)
            .or_log_error("Reticulate: Could not send close message to the front-end");

        // Reset the global soccket before closing
        log::info!("Reticulate Thread closing {:?}", self.comm.comm_id);
        {
            let mut outgoing_tx_guard = RETICULATE_OUTGOING_TX.lock().unwrap();
            *outgoing_tx_guard = None;
        }

        Ok(())
    }
}

// Creates a client instance reticulate can use to communicate with the front-end.
// We should aim at having at most **1** client per R session.
// Further actions that reticulate can ask the front-end can be requested through
// the comm_id that is returned by this function.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_reticulate_open(input: SEXP) -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();

    let input: RObject = input.try_into()?;
    // Reticulate sends `NULL` or a string with the code to be executed in the Python console.
    let input_code: Option<String> = if r_is_null(input.sexp) {
        None
    } else {
        Some(input.try_into()?)
    };

    // If there's an id already registered, we just need to send the focus event
    {
        let outgoing_tx_guard = RETICULATE_OUTGOING_TX.lock().unwrap();
        if let Some(outgoing_tx) = outgoing_tx_guard.deref() {
            // There's a comm_id registered, we just send the focus event
            outgoing_tx.send(CommMsg::Data(json!({
                "method": "focus",
                "params": {
                    "input": input_code
                }
            })))?;
            return Ok(R_NilValue);
        }
    }

    let id = format!("reticulate-{}", Uuid::new_v4().to_string());
    ReticulateService::start(id, main.get_comm_manager_tx().clone(), input_code)?;

    Ok(R_NilValue)
}
