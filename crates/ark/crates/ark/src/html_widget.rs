//
// html_widget.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::result::Result::Ok;

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;
use serde_json::Value;

use crate::interface::RMain;

#[harp::register]
pub unsafe extern "C" fn ps_html_widget(kind: SEXP, tags: SEXP) -> Result<SEXP, anyhow::Error> {
    // For friendly display: the class/kind of the widget
    let widget_class = String::try_from(RObject::view(kind))?;

    // Convert the tags to JSON for display
    let json = Value::try_from(RObject::view(tags))?;

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

    iopub_tx.send(message)?;

    Ok(R_NilValue)
}
