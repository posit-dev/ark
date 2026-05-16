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
use anyhow::Result;
use crossbeam::channel::Sender;
use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;

use crate::console::Console;
use crate::console::SessionMode;

/// Emit pre-rendered HTML output on IOPub for delivery to the client.
///
/// - `iopub_tx` - The IOPub channel to send the output on
/// - `contents` - The complete HTML document to emit as `text/html`
/// - `kind` - A short label used in the `text/plain` fallback
fn emit_html_output_jupyter(
    iopub_tx: Sender<IOPubMessage>,
    contents: String,
    kind: String,
) -> Result<()> {
    let output = serde_json::json!({
        "text/html": contents,
        "text/plain": format!("<{kind}>"),
    });

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
    destination: SEXP,
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
            let console = Console::get();
            let iopub_tx = console.iopub_tx().clone();
            match console.session_mode() {
                SessionMode::Notebook | SessionMode::Background => {
                    // In notebook mode, read the rendered HTML from disk and emit
                    // it as a Jupyter display_data message. Note that this path is
                    // only used for non-widget HTML viewing (e.g. `rstudioapi::viewer()`);
                    // widget printing renders self-contained HTML in R and goes
                    // through `ps_html_widget_emit` directly.
                    match std::fs::read_to_string(&path) {
                        Ok(contents) => {
                            if let Err(err) = emit_html_output_jupyter(iopub_tx, contents, label) {
                                log::error!("Failed to emit HTML output: {err:?}");
                            }
                        },
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
/// message. Called from R via `.ps.Call("ps_html_widget_emit", html, label)`.
///
/// Used by the notebook-mode widget print path, which renders the widget to a
/// single HTML string with its JS/CSS dependencies inlined as `data:` URIs
/// (see `crates/ark/src/modules/positron/html_widgets.R`). Console mode keeps
/// the temp-file flow via `ps_html_viewer`.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_html_widget_emit(
    html: SEXP,
    label: SEXP,
) -> anyhow::Result<SEXP> {
    let html = RObject::view(html).to::<String>()?;
    let label = match RObject::view(label).to::<String>() {
        Ok(label) => label,
        Err(_) => String::from("R"),
    };

    let console = Console::get();
    match console.session_mode() {
        SessionMode::Notebook | SessionMode::Background => {
            let iopub_tx = console.iopub_tx().clone();
            if let Err(err) = emit_html_output_jupyter(iopub_tx, html, label) {
                log::error!("Failed to emit HTML widget output: {err:?}");
            }
        },
        SessionMode::Console => {
            // R-side guards this call with `.ps.is_notebook()`; reaching this
            // branch indicates a logic error in the caller.
            log::warn!("ps_html_widget_emit called in console mode; ignoring");
        },
    }

    Ok(R_NilValue)
}

/// Returns `TRUE` when the kernel is running in a non-interactive output
/// context (notebook or background session), where rich output must be emitted
/// inline via `display_data` rather than routed through the Positron UI comm.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_is_notebook() -> anyhow::Result<SEXP> {
    let is_notebook = matches!(
        Console::get().session_mode(),
        SessionMode::Notebook | SessionMode::Background
    );
    Ok(libr::Rf_ScalarLogical(is_notebook as i32))
}
