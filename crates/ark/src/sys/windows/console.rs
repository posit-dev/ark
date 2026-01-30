/*
 * console.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::c_char;
use std::ffi::c_int;
use std::ffi::CStr;
use std::ffi::CString;
use std::mem::MaybeUninit;

use libr::cmdlineoptions;
use libr::get_R_HOME;
use libr::readconsolecfg;
use libr::run_Rmainloop;
use libr::setup_Rmainloop;
use libr::R_DefParamsEx;
use libr::R_HomeDir;
use libr::R_SetParams;
use libr::R_SignalHandlers;
use libr::R_common_command_line;
use libr::Rboolean_FALSE;
use once_cell::sync::Lazy;
use regex::bytes::Regex;

use super::strings::code_page_to_utf8;
use super::strings::get_system_code_page;
use crate::console::r_busy;
use crate::console::r_read_console;
use crate::console::r_show_message;
use crate::console::r_suicide;
use crate::console::r_write_console;
use crate::console::Console;
use crate::sys::windows::strings::system_to_utf8;

pub fn setup_r(args: &Vec<String>) {
    unsafe {
        libr::set(R_SignalHandlers, 0);

        // - We have to collect these before `cmdlineoptions()` is called, because
        //   it alters the env vars, which we then reset to our own paths in `R_SetParams()`.
        // - `rhome` and `home` need to be set before `R_SetParams()` because it accesses them.
        // - We convert to a `mut` pointer because the R API requires it, but it doesn't modify them.
        // - `CString::new()` handles adding a nul terminator for us.
        let r_home = get_r_home();
        let r_home = CString::new(r_home).unwrap();
        let r_home = r_home.as_ptr() as *mut c_char;

        let user_home = get_user_home();
        let user_home = CString::new(user_home).unwrap();
        let user_home = user_home.as_ptr() as *mut c_char;

        // Note that R does a lot of initialization here that's not accessible
        // in any other way; e.g. the default translation domain is set within
        // via `BindDomain()`. We don't supply the `args` here because we do a
        // wholesale replacement of the options they set via `R_SetParams()`
        // later on, so setting them here would have no effect anyways.
        // https://github.com/rstudio/rstudio/issues/10308
        let mut c_args = Console::build_ark_c_args(&vec![]);
        cmdlineoptions(c_args.len() as i32, c_args.as_mut_ptr() as *mut *mut c_char);

        let mut params_struct = MaybeUninit::uninit();
        let params: libr::Rstart = params_struct.as_mut_ptr();

        // Set up initial defaults for `params`
        //
        // TODO: Windows
        // We eventually need to use `RSTART_VERSION` (i.e., 1). It might just
        // work as is but will require a little testing. It sets and initializes
        // some additional useful callbacks, but is only available in newer R
        // versions.
        // R_DefParamsEx(params, bindings::RSTART_VERSION as i32);
        R_DefParamsEx(params, 0);

        // Set up "common" command line arguments, inheriting R's "last flag
        // wins" behavior for these. On the Unix side this is automatically
        // called by `Rf_initialize_R()`. On the Windows side this is called by
        // `cmdlineoptions()`, but because we call `R_SetParams()` later on to
        // tweak some options, we have to fully rebuild the correct `params`
        // list anyways so we don't supply any user `args` to
        // `cmdlineoptions()` and instead pass them here.
        //
        // Notably sets:
        // - `(*params).R_Quiet` via `--silent`, `--quiet`, `-q`, `--no-echo`
        // - `(*params).R_Verbose` via `--verbose`
        // - `(*params).NoRenviron` via `--no-environ`, `--vanilla`
        // - `(*params).SaveAction` via `--save`, `--no-save`, `--vanilla`, `--no-echo`
        // - `(*params).RestoreAction` via `--restore`, `--no-restore`, `--no-restore-data`, `--vanilla`
        // - `R_RestoreHistory` (a global) via `--restore`, `--no-restore`, `--no-restore-history`, `--vanilla`
        let mut c_args = Console::build_ark_c_args(args);
        let mut c_args_len = c_args.len() as std::ffi::c_int;
        R_common_command_line(
            &mut c_args_len,
            c_args.as_mut_ptr() as *mut *mut c_char,
            params,
        );

        (*params).R_Interactive = 1;
        (*params).CharacterMode = libr::UImode_RGui;

        // Never load the user or site `.Rprofile`s during `setup_Rmainloop()`.
        // We do it for the user once ark is ready. We faithfully reimplement
        // R's behavior for finding these files in `startup.rs`.
        (*params).LoadInitFile = Rboolean_FALSE;
        (*params).LoadSiteFile = Rboolean_FALSE;

        (*params).WriteConsole = None;
        (*params).WriteConsoleEx = Some(r_write_console);
        (*params).ReadConsole = Some(r_read_console);
        (*params).ShowMessage = Some(r_show_message);
        (*params).YesNoCancel = Some(r_yes_no_cancel);
        (*params).Busy = Some(r_busy);
        (*params).Suicide = Some(r_suicide);

        // This is assigned to `ptr_ProcessEvents` (which we don't set on Unix),
        // in `R_SetParams()` by `R_SetWin32()` and gets called by `R_ProcessEvents()`.
        // It gets called unconditionally, so we have to set it to something, even if a no-op.
        (*params).CallBack = Some(r_callback);

        (*params).rhome = r_home;
        (*params).home = user_home;

        // Sets the parameters to internal R globals, like all of the `ptr_*` function pointers
        R_SetParams(params);

        // In tests R may be run from various threads. This confuses R's stack
        // overflow checks so we disable those. This should not make it in
        // production builds as it causes stack overflows to crash R instead of
        // throwing an R error.
        if stdext::IS_TESTING {
            libr::set(libr::R_CStackLimit, usize::MAX);
        }

        // R global ui initialization
        libr::graphapp::GA_initapp(0, std::ptr::null_mut());
        readconsolecfg();

        // Log the value of R_HOME, so we can know if something hairy is afoot
        let home = CStr::from_ptr(R_HomeDir());
        log::trace!("R_HOME: {:?}", home);

        // Set up main loop
        setup_Rmainloop();
    }
}

pub fn run_r() {
    unsafe {
        run_Rmainloop();
    }
}

pub fn run_activity_handlers() {
    // Nothing to do on Windows
}

// TODO: Windows
// It is possible we will want to use something other than `get_R_HOME()` and `getRUser()` for these.
// RStudio does use `get_R_HOME()`, but they have a custom helper instead of `getRUser()`.
// https://github.com/rstudio/rstudio/blob/d9c0b090d49752fe60e7a2ea4be3123cc3feeb6c/src/cpp/r/session/RDiscovery.cpp#L42
// https://github.com/rstudio/rstudio/blob/d9c0b090d49752fe60e7a2ea4be3123cc3feeb6c/src/cpp/shared_core/system/Win32User.cpp#L164
fn get_r_home() -> String {
    let r_path = unsafe { get_R_HOME() };

    if r_path.is_null() {
        panic!("`get_R_HOME()` failed to report an R home.");
    }

    let r_path_ctr = unsafe { CStr::from_ptr(r_path) };

    // Removes nul terminator
    let path = r_path_ctr.to_bytes();

    // Try conversion to UTF-8
    let path = match system_to_utf8(path) {
        Ok(path) => path,
        Err(err) => {
            let path = r_path_ctr.to_string_lossy().to_string();
            panic!("Failed to convert R home to UTF-8. Path '{path}'. Error: {err:?}.");
        },
    };

    path.to_string()
}

fn get_user_home() -> String {
    let r_path = unsafe { libr::getRUser() };

    if r_path.is_null() {
        panic!("`getRUser()` failed to report a user home directory.");
    }

    let r_path_ctr = unsafe { CStr::from_ptr(r_path) };

    // Removes nul terminator
    let path = r_path_ctr.to_bytes();

    // Try conversion to UTF-8
    let path = match system_to_utf8(path) {
        Ok(path) => path,
        Err(err) => {
            let path = r_path_ctr.to_string_lossy().to_string();
            panic!("Failed to convert user home to UTF-8. Path '{path}'. Error: {err:?}.");
        },
    };

    path.to_string()
}

#[no_mangle]
extern "C-unwind" fn r_callback() {
    // Do nothing!
}

#[no_mangle]
extern "C-unwind" fn r_yes_no_cancel(question: *const c_char) -> c_int {
    // This seems to only be used on Windows during R's default `CleanUp` when
    // `SA_SAVEASK` is used. We should replace `Cleanup` with our own version
    // that resolves `SA_SAVEASK`, changes `saveact` to the resolved value,
    // then calls R's default `CleanUp` with the new value. That way this never
    // gets called (at which point we can make this an error). In the meantime
    // we simply return `-1` to request "no save" on exit.
    // https://github.com/wch/r-source/blob/bd5e9741c9b55c481a2e5d4f929cf1597ec08fcc/src/gnuwin32/system.c#L565
    let question = unsafe { CStr::from_ptr(question).to_str().unwrap() };
    log::warn!("Ignoring `YesNoCancel` question: '{question}'. Returning `NO`.");
    return -1;
}

// - (?-u) to disable unicode so it matches the bytes exactly
// - (?s:.) so `.` matches anything INCLUDING new lines
// https://github.com/rust-lang/regex/blob/837fd85e79fac2a4ea64030411b9a4a7b17dfa42/src/builders.rs#L368-L372
static RE_EMBEDDED_UTF8: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?-u)\x02\xFF\xFE(?<text>(?s:.)*?)\x03\xFF\xFE").unwrap());

/// NOTE: On Windows with GUIs, when R attempts to write text to
/// the console, it will surround UTF-8 text with 3-byte escapes:
///
///    \002\377\376 <text> \003\377\376
///
/// strangely, we see these escapes around text that is not UTF-8
/// encoded but rather is encoded according to the active locale.
/// extract those pieces of text (discarding the escapes) and
/// convert to UTF-8. (still not exactly sure what the cause of this
/// behavior is; perhaps there is an extra UTF-8 <-> system conversion
/// happening somewhere in the pipeline?)
pub fn console_to_utf8(x: *const c_char) -> anyhow::Result<String> {
    let code_page = get_system_code_page();

    let x = unsafe { CStr::from_ptr(x) };

    // Drops trailing nul terminator
    let mut x = x.to_bytes();

    let mut out = Vec::new();

    while let Some(capture) = RE_EMBEDDED_UTF8.captures(x) {
        // `get(0)` always returns the full match
        let full = capture.get(0).unwrap();

        if full.start() > 0 {
            // Translate everything up to right before the match
            // and add to the output
            let slice = code_page_to_utf8(&x[..full.start()], code_page)?;
            out.push(slice);
        }

        // Add everything in the `text` capture group.
        // By definition, this is already UTF-8.
        let text = capture.name("text").unwrap().as_bytes();
        let text = std::str::from_utf8(text).unwrap();
        let text = text.to_string();
        out.push(text);

        // Advance `x`
        x = &x[full.end()..];
    }

    if x.len() > 0 {
        // Translate everything that's left and add to the output
        let slice = code_page_to_utf8(x, code_page)?;
        out.push(slice);
    }

    let out = out.join("");

    Ok(out)
}
