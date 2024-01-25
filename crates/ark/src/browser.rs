//
// browser.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::process::Command;

use anyhow::Result;
use harp::object::RObject;
use libr::Rf_ScalarLogical;
use libr::SEXP;

use crate::help::message::HelpReply;
use crate::help::message::HelpRequest;
use crate::interface::RMain;

pub static mut PORT: u16 = 0;

#[harp::register]
pub unsafe extern "C" fn ps_browse_url(url: SEXP) -> Result<SEXP> {
    ps_browse_url_impl(url).or_else(|err| {
        log::error!("{err:?}");
        Ok(Rf_ScalarLogical(0))
    })
}

unsafe fn handle_help_url(url: &str) -> Result<bool> {
    let main = RMain::get();
    let help_tx = &main.help_tx;

    let Some(help_tx) = help_tx else {
        log::error!(
            "No help channel available to handle help URL {}. Is the help comm open?",
            url
        );
        return Ok(false);
    };

    let message = HelpRequest::ShowHelpUrlRequest(url.to_string());

    if let Err(err) = help_tx.send(message) {
        log::error!("Failed to send help message: {err:?}");
        return Ok(false);
    }

    // Wait up to 1 second for a reply from the help thread
    let reply = main
        .help_rx
        .as_ref()
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(1))?;

    match reply {
        HelpReply::ShowHelpUrlReply(found) => Ok(found),
    }
}

unsafe fn ps_browse_url_impl(url: SEXP) -> Result<SEXP> {
    // Extract URL.
    let url = RObject::view(url).to::<String>()?;

    // Handle help server requests.
    if handle_help_url(&url)? {
        return Ok(Rf_ScalarLogical(1));
    }

    // TODO: What should we do with other URLs? This is used for opening,
    // for example, web applications (e.g. Shiny) and also interactive plots
    // (e.g. htmlwidgets).
    Command::new("open").arg(url).output()?;
    Ok(Rf_ScalarLogical(1))
}
