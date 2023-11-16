//
// html_widget.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::result::Result::Ok;

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use anyhow::*;
use harp::exec::r_unwrap;
use harp::object::RObject;
use libR_sys::R_NilValue;
use libR_sys::SEXP;
use log::warn;
use serde_json::Value;

use crate::interface::RMain;
#[harp::register]
pub unsafe extern "C" fn ps_html_widget(kind: SEXP, tags: SEXP) -> SEXP {
    // For friendly display: the class/kind of the widget
    let widget_class = match String::try_from(RObject::view(kind)) {
        Ok(kind) => kind,
        Err(err) => {
            warn!("Failed to convert HTML widget class to string: {:?}", err);
            String::new()
        },
    };

    // Convert the tags to JSON for display
    let json = r_unwrap(|| match Value::try_from(RObject::view(tags)) {
        Ok(val) => Ok(val),
        Err(err) => Err(anyhow!(err).context("Failed to convert HTML widget tags to JSON")),
    });

    // Get the IOPub channel
    let main = RMain::get();
    let iopub_tx = main.get_iopub_tx().clone();

    // Create the output object
    let output = serde_json::json!({
        "application/vnd.r.htmlwidget": json,
        "text/plain": format!("<{} HTML widget>", widget_class)
    });

    // Emit the HTML output on IOPub for delivery to the client
    let message = IOPubMessage::DisplayData(DisplayData {
        data: output,
        metadata: serde_json::Value::Null,
        transient: serde_json::Value::Null,
    });
    iopub_tx.send(message).unwrap();

    R_NilValue
}
