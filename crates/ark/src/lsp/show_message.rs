//
// show_message.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::ui_comm::ShowMessageParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;
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
    let event = UiFrontendEvent::ShowMessage(ShowMessageParams { message });

    let kernel = main.get_kernel().lock().unwrap();
    kernel.send_ui_event(event);

    Ok(R_NilValue)
}
