//
// show_message.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::events::PositronEvent;
use amalthea::events::ShowMessageEvent;
use harp::object::RObject;
use libR_sys::*;
use stdext::local;
use stdext::unwrap;

use crate::lsp::globals::shell_request_tx;
use crate::request::Request;

/// Shows a message in the Positron frontend
#[harp::register]
pub unsafe extern "C" fn ps_show_message(message: SEXP) -> SEXP {
    let result: anyhow::Result<()> = local! {
        // Convert message to a string
        let message = RObject::view(message).to::<String>()?;

        // Get the global instance of the channel used to deliver requests to the
        // front end, and send a request to show the message
        let event = PositronEvent::ShowMessage(ShowMessageEvent { message });
        let event = Request::DeliverEvent(event);

        let shell_request_tx = shell_request_tx();
        let status = unwrap!(shell_request_tx.send(event), Err(error) => {
            anyhow::bail!("Error sending request: {}", error);
        });

        Ok(status)
    };

    let _result = unwrap!(result, Err(error) => {
        log::error!("{}", error);
        return Rf_ScalarLogical(0);
    });

    Rf_ScalarLogical(1)
}
