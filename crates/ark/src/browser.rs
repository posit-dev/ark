//
// browser.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::ui_comm::ShowUrlParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use harp::object::RObject;
use libr::Rf_ScalarLogical;
use libr::SEXP;

use crate::help::message::HelpEvent;
use crate::help::message::ShowHelpUrlParams;
use crate::interface::RMain;

pub static mut PORT: u16 = 0;

#[harp::register]
pub unsafe extern "C" fn ps_browse_url(url: SEXP) -> anyhow::Result<SEXP> {
    ps_browse_url_impl(url).or_else(|err| {
        log::error!("Failed to browse url due to: {err}");
        Ok(Rf_ScalarLogical(0))
    })
}

fn is_help_url(url: &str) -> bool {
    RMain::with(|main| main.is_help_url(url))
}

fn handle_help_url(url: String) -> anyhow::Result<()> {
    RMain::with(|main| {
        let event = HelpEvent::ShowHelpUrl(ShowHelpUrlParams { url });
        main.send_help_event(event)
    })
}

unsafe fn ps_browse_url_impl(url: SEXP) -> anyhow::Result<SEXP> {
    // Extract URL.
    let url = RObject::view(url).to::<String>()?;
    let _span = tracing::trace_span!("browseURL", url = %url).entered();

    // Handle help server requests.
    if is_help_url(&url) {
        log::trace!("Help is handling URL");
        handle_help_url(url)?;
        return Ok(Rf_ScalarLogical(1));
    } else {
        log::trace!("Help is not handling URL");
    }

    // For all other URLs, create a ShowUrl event and send it to the main
    // thread; Positron will handle it.
    let params = ShowUrlParams { url };
    let event = UiFrontendEvent::ShowUrl(params);

    RMain::with(|main| main.send_frontend_event(event));

    Ok(Rf_ScalarLogical(1))
}
