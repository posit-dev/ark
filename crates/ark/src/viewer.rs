//
// viewer.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::ui_comm::ShowHtmlFileDestination;
use amalthea::comm::ui_comm::ShowHtmlFileParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use crossbeam::channel::Sender;
use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;

use crate::console::Console;
use crate::console::SessionMode;

#[harp::register]
pub unsafe extern "C-unwind" fn ps_html_viewer(
    url: SEXP,
    label: SEXP,
    height: SEXP,
    destination: SEXP,
) -> anyhow::Result<SEXP> {
    let console = Console::get();

    // Convert url to a string; note that we are only passed URLs that
    // correspond to files in the temporary directory.
    let path = RObject::view(url).to::<String>();
    let label = match RObject::view(label).to::<String>() {
        Ok(label) => label,
        Err(_) => String::from("R"),
    };

    match path {
        Ok(path) => {
            match console.session_mode() {
                SessionMode::Notebook | SessionMode::Background => {
                    // In notebook mode, read the rendered HTML from disk and emit
                    // it as a Jupyter `display_data` message
                    match std::fs::read_to_string(&path) {
                        Ok(contents) => emit_html_display_data(contents, label, console.iopub_tx()),
                        Err(err) => {
                            log::error!("Failed to read HTML file {path}: {err:?}");
                        },
                    }
                },
                SessionMode::Console => {
                    let destination = match RObject::view(destination).to::<String>() {
                        Ok(s) => s.parse::<ShowHtmlFileDestination>().unwrap_or_else(|_| {
                            log::warn!(
                                "`destination` must be one of 'plot', 'editor', or 'viewer', using 'viewer' as a fallback."
                            );
                            ShowHtmlFileDestination::Viewer
                        }),
                        Err(err) => {
                            log::warn!(
                                "Can't convert `destination` to a string, using 'viewer' as a fallback: {err}"
                            );
                            ShowHtmlFileDestination::Viewer
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
                        destination,
                    };

                    let event = UiFrontendEvent::ShowHtmlFile(params);

                    // TODO: What's the right thing to do in `Console` mode when
                    // we aren't connected to Positron? Right now we error.
                    console.try_ui_comm()?.send_event(&event);
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

/// Emit a pre-rendered, self-contained HTML widget as a Jupyter `display_data`
/// message.
///
/// Used by the notebook-mode widget print path, which renders the widget to a
/// single HTML string with its JS/CSS dependencies inlined as `data:` URIs.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_html_display_data(
    html: SEXP,
    label: SEXP,
) -> anyhow::Result<SEXP> {
    let console = Console::get();

    let html = RObject::view(html).to::<String>()?;
    let label = match RObject::view(label).to::<String>() {
        Ok(label) => label,
        Err(_) => String::from("R"),
    };

    emit_html_display_data(html, label, console.iopub_tx());

    Ok(R_NilValue)
}

fn emit_html_display_data(contents: String, kind: String, iopub_tx: &Sender<IOPubMessage>) {
    let data = serde_json::json!({
        "text/html": contents,
        "text/plain": format!("<{kind}>"),
    });

    let message = IOPubMessage::DisplayData(DisplayData {
        data,
        metadata: serde_json::Value::Null,
        transient: serde_json::Value::Null,
    });

    if let Err(err) = iopub_tx.send(message) {
        log::error!("Failed to emit HTML output: {err:?}");
    }
}
