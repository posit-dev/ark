//
// show_message.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::frontend_comm::FrontendEvent;
use amalthea::comm::frontend_comm::ShowMessageParams;
use harp::object::RObject;
use libR_shim::*;
use stdext::unwrap;

use crate::interface::RMain;

/// Shows a message in the Positron frontend
///
/// Test helper for `R_ShowMessage()` support
#[harp::register]
pub unsafe extern "C" fn ps_show_message(message: SEXP) -> anyhow::Result<SEXP> {
    // Convert message to a string
    let message = unwrap!(RObject::view(message).to::<String>(), Err(error) => {
        log::error!("Failed to convert `message` to a string: {error:?}.");
        return Ok(R_NilValue);
    });

    let main = RMain::get();

    // Send a request to show the message
    let event = FrontendEvent::ShowMessage(ShowMessageParams { message });

    let kernel = main.get_kernel().lock().unwrap();
    kernel.send_event(event);

    Ok(R_NilValue)
}
