//
// startup.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;
use std::str::FromStr;

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::stream::Stream;
use amalthea::wire::stream::StreamOutput;
use harp::call::RCall;
use harp::exec::r_top_level_exec;
use harp::r_symbol;
use libr::Rf_eval;

use crate::interface::RMain;
use crate::modules::ARK_ENVS;
use crate::sys;

pub(crate) fn should_ignore_site_r_profile(args: &Vec<String>) -> bool {
    args.iter()
        .any(|arg| arg == "--no-site-file" || arg == "--vanilla")
}

pub(crate) fn should_ignore_user_r_profile(args: &Vec<String>) -> bool {
    args.iter()
        .any(|arg| arg == "--no-init-file" || arg == "--vanilla")
}

pub(crate) fn push_ignore_site_r_profile(args: &mut Vec<String>) {
    args.push(String::from("--no-site-file"))
}

pub(crate) fn push_ignore_user_r_profile(args: &mut Vec<String>) {
    args.push(String::from("--no-init-file"))
}

// Mimics `R_OpenSiteFile()`
// https://github.com/wch/r-source/blob/ee6b15303be885d118d49b441e32a9cff5cda778/src/main/startup.c#L96
pub(crate) fn source_site_r_profile(r_home: &PathBuf) {
    match find_site_r_profile(r_home) {
        Some(path) => source_r_profile(&path),
        None => (),
    }
}

// Mimics `R_OpenInitFile()`
// Windows: https://github.com/wch/r-source/blob/ee6b15303be885d118d49b441e32a9cff5cda778/src/gnuwin32/sys-win32.c#L40
// Unix: https://github.com/wch/r-source/blob/ee6b15303be885d118d49b441e32a9cff5cda778/src/unix/sys-unix.c#L68
pub(crate) fn source_user_r_profile() {
    match find_user_r_profile() {
        Some(path) => source_r_profile(&path),
        None => (),
    }
}

fn source_r_profile(path: &PathBuf) {
    let path = path.to_string_lossy().to_string();
    let path = path.as_str();

    log::info!("Found R profile at '{path}', sourcing now");

    // Must source with `r_top_level_exec()`. In particular, can't source with the typical
    // `r_safe_eval()` because it wraps in `withCallingHandlers()`, which prevents
    // `globalCallingHandlers()` from being called within `.Rprofile`s (can't call it when
    // there are handlers on the stack). That is a common place to register global calling
    // handlers, including in Gabor's prompt package.
    let result = unsafe {
        let call = RCall::new(r_symbol!(".ps.source_r_profile"))
            .param("path", path)
            .build();
        r_top_level_exec(|| Rf_eval(call.sexp, ARK_ENVS.positron_ns))
    };

    let Err(err) = result else {
        log::info!("Successfully sourced R profile at '{path}'");
        return;
    };

    log::error!("Error while sourcing R profile at '{path}': {err}");

    let harp::Error::TopLevelExecError { message, .. } = err else {
        unreachable!("Only `TopLevelExecError` errors should be thrown.");
    };

    // Forward the message on to the frontend to be shown in the console.
    // This technically happens outside of any parent context, but that is allowed.
    let message = format!("Error while sourcing R profile file at path '{path}':\n{message}");

    let message = IOPubMessage::Stream(StreamOutput {
        name: Stream::Stderr,
        text: message,
    });

    RMain::with(|main| main.get_iopub_tx().send(message).unwrap())
}

fn find_site_r_profile(r_home: &PathBuf) -> Option<PathBuf> {
    // Try from env var first
    match std::env::var("R_PROFILE") {
        Ok(path) => return PathBuf::from_str(path.as_str()).ok(),
        Err(_) => (),
    };

    // Then from specified location under R's home directory
    // TODO: Need etc/x64 on Windows?
    let path = r_home.join("etc").join("Rprofile.site");
    if path.exists() {
        return Some(path);
    }

    None
}

fn find_user_r_profile() -> Option<PathBuf> {
    // Try from env var first
    match std::env::var("R_PROFILE_USER") {
        Ok(path) => return PathBuf::from_str(path.as_str()).ok(),
        Err(_) => (),
    };

    // Then from current directory level `.Rprofile`
    match std::env::current_dir().map(|dir| dir.join(".Rprofile")) {
        Ok(path) => return Some(path),
        Err(_) => {
            // Swallow any errors and try other sources
            ()
        },
    }

    // Then from user level home `.Rprofile`
    match sys::path::r_user_home().map(|dir| dir.join(".Rprofile")) {
        Some(path) => return Some(path),
        None => (),
    }

    None
}
