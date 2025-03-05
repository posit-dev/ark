//
// viewer.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::ui_comm::ShowHtmlFileParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
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
pub unsafe extern "C-unwind" fn ps_html_viewer(
    url: SEXP,
    label: SEXP,
    height: SEXP,
    is_plot: SEXP,
) -> anyhow::Result<SEXP> {
    // Convert url to a string; note that we are only passed URLs that
    // correspond to files in the temporary directory.
    let path = RObject::view(url).to::<String>();
    let label = match RObject::view(label).to::<String>() {
        Ok(label) => label,
        Err(_) => String::from("R"),
    };

    match path {
        Ok(path) => {
            // Emit HTML output
            let main = RMain::get();
            let iopub_tx = main.get_iopub_tx().clone();
            match main.session_mode() {
                SessionMode::Notebook | SessionMode::Background => {
                    // In notebook mode, send the output as a Jupyter display_data message
                    if let Err(err) = emit_html_output_jupyter(iopub_tx, path, label) {
                        log::error!("Failed to emit HTML output: {:?}", err);
                    }
                },
                SessionMode::Console => {
                    let is_plot = RObject::view(is_plot).to::<bool>();
                    let is_plot = match is_plot {
                        Ok(is_plot) => is_plot,
                        Err(err) => {
                            log::warn!("Can't convert `is_plot` into a bool, using `false` as a fallback: {err:?}");
                            false
                        },
                    };

                    let height = RObject::view(height).to::<i32>();
                    let height = match height {
                        Ok(height) => height.into(),
                        Err(err) => {
                            log::warn!("Can't convert `height` into an i32, using `0` as a fallback: {err:?}");
                            0
                        },
                    };

                    let params = ShowHtmlFileParams {
                        path,
                        title: label,
                        height,
                        is_plot,
                    };

                    let event = UiFrontendEvent::ShowHtmlFile(params);

                    // TODO: What's the right thing to do in `Console` mode when
                    // we aren't connected to Positron? Right now we error.
                    let ui_comm_tx = main
                        .get_ui_comm_tx()
                        .ok_or_else(|| anyhow::anyhow!("UI comm not connected."))?;

                    ui_comm_tx.send_event(event);
                },
            }
        },
        Err(err) => {
            log::error!("Attempt to view invalid path {:?}: {:?}", url, err);
        },
    }

    // No return value
    Ok(R_NilValue)
}
