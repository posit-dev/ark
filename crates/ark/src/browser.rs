//
// browser.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::process::Command;

use anyhow::Result;
use harp::exec::RFunction;
use harp::object::RObject;
use libR_sys::*;

use crate::help::message::HelpRequest;
use crate::interface::R_MAIN;

pub static mut PORT: u16 = 0;

#[harp::register]
pub unsafe extern "C" fn ps_browse_url(url: SEXP) -> SEXP {
    match ps_browse_url_impl(url) {
        Ok(_) => Rf_ScalarLogical(1),
        Err(error) => {
            log::error!("{}", error);
            Rf_ScalarLogical(0)
        },
    }
}

unsafe fn handle_help_url(url: &str) -> Result<bool> {
    // Check for help URLs
    let port = RFunction::new("tools", "httpdPort").call()?.to::<u16>()?;
    let prefix = format!("http://127.0.0.1:{}/", port);
    if !url.starts_with(&prefix) {
        return Ok(false);
    }

    // Re-direct the help request to our help proxy server.
    let replacement = format!("http://127.0.0.1:{}/", PORT);

    // Fire an event for the front-end.
    let url = url.replace(prefix.as_str(), replacement.as_str());

    let main = R_MAIN.as_ref().unwrap();
    let help = &main.help_tx;

    if let Some(help) = help {
        if let Err(err) = help.send(HelpRequest::ShowHelpUrl(url)) {
            log::error!("Failed to send help message: {}", err);
        }
    }

    Ok(true)
}

unsafe fn ps_browse_url_impl(url: SEXP) -> Result<()> {
    // Extract URL.
    let url = RObject::view(url).to::<String>()?;

    // Handle help server requests.
    if handle_help_url(&url)? {
        return Ok(());
    }

    // TODO: What should we do with other URLs? This is used for opening,
    // for example, web applications (e.g. Shiny) and also interactive plots
    // (e.g. htmlwidgets).
    Command::new("open").arg(url).output()?;
    Ok(())
}
