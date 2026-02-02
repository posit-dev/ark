//
// browser.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use harp::object::RObject;
use harp::utils::r_normalize_path;
use libr::Rf_ScalarLogical;
use libr::SEXP;

use crate::console::Console;
use crate::help::message::HelpEvent;
use crate::help::message::ShowHelpUrlKind;
use crate::help::message::ShowHelpUrlParams;
use crate::ui::events::send_open_with_system_event;
use crate::ui::events::send_show_url_event;

#[harp::register]
pub unsafe extern "C-unwind" fn ps_browse_url(url: SEXP) -> anyhow::Result<SEXP> {
    ps_browse_url_impl(url).or_else(|err| {
        log::error!("Failed to browse url due to: {err}");
        Ok(Rf_ScalarLogical(0))
    })
}

fn is_help_url(url: &str) -> bool {
    Console::get().is_help_url(url)
}

fn handle_help_url(url: String) -> anyhow::Result<()> {
    let event = HelpEvent::ShowHelpUrl(ShowHelpUrlParams {
        url,
        kind: ShowHelpUrlKind::HelpProxy,
    });
    Console::get().send_help_event(event)
}

unsafe fn ps_browse_url_impl(url: SEXP) -> anyhow::Result<SEXP> {
    // Extract URL string for analysis
    let url_string = RObject::view(url).to::<String>()?;
    let _span = tracing::trace_span!("browseURL", url = %url_string).entered();

    // Handle help server requests.
    if is_help_url(&url_string) {
        log::trace!("Help is handling URL");
        handle_help_url(url_string)?;
        return Ok(Rf_ScalarLogical(1));
    }

    // Handle web URLs
    if is_web_url(&url_string) {
        log::trace!("Handling web URL");
        send_show_url_event(&url_string)?;
        return Ok(Rf_ScalarLogical(1));
    }

    // This is probably a file path? Send to the front end and ask for system
    // default opener.
    log::trace!("Treating as file path and asking system to open");
    let path = r_normalize_path(url.into())?;
    send_open_with_system_event(&path)?;
    Ok(Rf_ScalarLogical(1))
}

fn is_web_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}
