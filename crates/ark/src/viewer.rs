//
// viewer.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::ui_comm::ShowHtmlFileParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use amalthea::wire::stream::Stream;
use amalthea::wire::stream::StreamOutput;
use anyhow::Result;
use crossbeam::channel::Sender;
use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;

use crate::interface::RMain;
use crate::interface::SessionMode;

/// Emit HTML output on IOPub for delivery to the client
///
/// - `iopub_tx` - The IOPub channel to send the output on
/// - `path` - The path to the HTML file to display
/// - `kind` - The kind of the HTML widget
fn emit_html_output_jupyter(
    iopub_tx: Sender<IOPubMessage>,
    path: String,
    kind: String,
) -> Result<()> {
    // Read the contents of the file
    let contents = std::fs::read_to_string(path)?;

    // Create the output object
    let output = serde_json::json!({
        "text/html": contents,
        "text/plain": format!("<{} HTML Widget>", kind),
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
pub unsafe extern "C" fn ps_html_viewer(
    url: SEXP,
    kind: SEXP,
    height: SEXP,
    is_plot: SEXP,
) -> anyhow::Result<SEXP> {
    // Convert url to a string; note that we are only passed URLs that
    // correspond to files in the temporary directory.
    let path = RObject::view(url).to::<String>();
    let kind = match RObject::view(kind).to::<String>() {
        Ok(kind) => kind,
        Err(_) => String::from("R"),
    };
    match path {
        Ok(path) => {
            // Emit HTML output
            let main = RMain::get();
            let iopub_tx = main.get_iopub_tx().clone();
            if main.session_mode == SessionMode::Notebook {
                // In notebook mode, send the output as a Jupyter display_data message
                if let Err(err) = emit_html_output_jupyter(iopub_tx, path, kind) {
                    log::error!("Failed to emit HTML output: {:?}", err);
                }
            } else {
                let is_plot = RObject::view(is_plot).to::<bool>();
                let height = RObject::view(height).to::<i32>();
                let params = ShowHtmlFileParams {
                    path,
                    kind: kind.clone(),
                    height: match height {
                        Ok(height) => height.into(),
                        Err(_) => 0,
                    },
                    is_plot: match is_plot {
                        Ok(plot) => plot,
                        Err(_) => false,
                    },
                };
                main.send_frontend_event(UiFrontendEvent::ShowHtmlFile(params));

                let message = IOPubMessage::Stream(StreamOutput {
                    name: Stream::Stdout,
                    text: format!("<{}>", kind),
                });
                iopub_tx.send(message)?;
            }
        },
        Err(err) => {
            log::error!("Attempt to view invalid path {:?}: {:?}", url, err);
        },
    }

    // No return value
    Ok(R_NilValue)
}
