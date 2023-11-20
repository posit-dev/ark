//
// viewer.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use anyhow::Result;
use crossbeam::channel::Sender;
use harp::object::RObject;
use libR_sys::R_NilValue;
use libR_sys::SEXP;

use crate::interface::RMain;

/// Emit HTML output on IOPub for delivery to the client
///
/// - `iopub_tx` - The IOPub channel to send the output on
/// - `path` - The path to the HTML file to display
fn emit_html_output(iopub_tx: Sender<IOPubMessage>, path: String) -> Result<()> {
    // Read the contents of the file
    let contents = std::fs::read_to_string(path)?;

    // Create the output object
    let output = serde_json::json!({
        "text/html": contents,
        "text/plain": String::from("<R HTML Widget>"),
    });

    // Emit the HTML output on IOPub for delivery to the client
    let message = IOPubMessage::DisplayData(DisplayData {
        data: output,
        metadata: serde_json::Value::Null,
        transient: serde_json::Value::Null,
    });
    iopub_tx.send(message)?;

    Ok(())
}

#[harp::register]
pub unsafe extern "C" fn ps_html_viewer(url: SEXP) -> SEXP {
    // Convert url to a string; note that we are only passed URLs that
    // correspond to files in the temporary directory.
    let path = RObject::view(url).to::<String>();
    match path {
        Ok(path) => {
            // Emit the HTML output
            let main = RMain::get();
            let iopub_tx = main.get_iopub_tx().clone();
            if let Err(err) = emit_html_output(iopub_tx, path) {
                log::error!("Failed to emit HTML output: {:?}", err);
            }
        },
        Err(err) => {
            log::error!("Attempt to view invalid path {:?}: {:?}", url, err);
        },
    }

    // No return value
    Ok(R_NilValue)
}
