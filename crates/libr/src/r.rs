//
// r.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

// Currently just using libR for the _types_, otherwise we conflict with it
pub use libR_shim::Rboolean;
pub use libR_shim::Rboolean_FALSE;
pub use libR_shim::Rboolean_TRUE;
pub use libR_shim::SEXP;

use crate::constant_globals;
use crate::functions;
use crate::mutable_globals;
// Reexport all system specific R types
pub use crate::sys::types::*;

// ---------------------------------------------------------------------------------------
// Functions and globals

functions::generate! {
    pub fn Rf_initialize_R(ac: std::ffi::c_int, av: *mut *mut std::ffi::c_char) -> std::ffi::c_int;

    pub fn run_Rmainloop();
    pub fn setup_Rmainloop();

    pub fn R_HomeDir() -> *mut std::ffi::c_char;

    pub fn R_removeVarFromFrame(symbol: SEXP, envir: SEXP);

    /// R >= 4.2.0
    pub fn R_existsVarInFrame(rho: SEXP, symbol: SEXP) -> Rboolean;

    // -----------------------------------------------------------------------------------
    // Unix

    /// NOTE: `R_checkActivity()` doesn't really return a void pointer, it returns
    /// a `*fd_set`. But because we never introspect these values directly and they're
    /// always passed around in R as pointers, it suffices to just use void pointers.
    #[cfg(target_family = "unix")]
    pub fn R_checkActivity(usec: i32, ignore_stdin: i32) -> *const std::ffi::c_void;

    /// NOTE: `R_runHandlers()` doesn't really take void pointers. But because we never
    /// introspect these values directly and they're always passed around in R as
    /// pointers, it suffices to just use void pointers.
    #[cfg(target_family = "unix")]
    pub fn R_runHandlers(handlers: *const std::ffi::c_void, fdset: *const std::ffi::c_void);

    // -----------------------------------------------------------------------------------
    // Windows

    #[cfg(target_family = "windows")]
    pub fn cmdlineoptions(ac: i32, av: *mut *mut std::ffi::c_char);

    #[cfg(target_family = "windows")]
    pub fn readconsolecfg();

    /// R >= 4.2.0
    #[cfg(target_family = "windows")]
    pub fn R_DefParamsEx(Rp: Rstart, RstartVersion: i32);

    #[cfg(target_family = "windows")]
    pub fn R_SetParams(Rp: Rstart);

    /// Get R_HOME from the environment or the registry
    ///
    /// Checks:
    /// - C `R_HOME` env var
    /// - Windows API `R_HOME` environment space
    /// - Current user registry
    /// - Local machine registry
    ///
    /// Probably returns a system encoded result?
    /// So needs to be converted to UTF-8.
    ///
    /// https://github.com/wch/r-source/blob/55cd975c538ad5a086c2085ccb6a3037d5a0cb9a/src/gnuwin32/rhome.c#L152
    #[cfg(target_family = "windows")]
    pub fn get_R_HOME() -> *mut std::ffi::c_char;

    /// Get user home directory
    ///
    /// Checks:
    /// - C `R_USER` env var
    /// - C `USER` env var
    /// - `Documents` folder, if one exists, through `ShellGetPersonalDirectory()`
    /// - `HOMEDRIVE` + `HOMEPATH`
    /// - Current directory through `GetCurrentDirectory()`
    ///
    /// Probably returns a system encoded result?
    /// So needs to be converted to UTF-8.
    ///
    /// https://github.com/wch/r-source/blob/55cd975c538ad5a086c2085ccb6a3037d5a0cb9a/src/gnuwin32/shext.c#L55
    #[cfg(target_family = "windows")]
    pub fn getRUser() -> *mut std::ffi::c_char;

    // In theory we should call these, but they are very new, roughly R 4.3.0.
    // It isn't super harmful if we don't free these.
    // https://github.com/wch/r-source/commit/9210c59281e7ab93acff9f692c31b83d07a506a6
    // pub fn freeRUser(s: *mut ::std::os::raw::c_char);
    // pub fn free_R_HOME(s: *mut ::std::os::raw::c_char);
}

constant_globals::generate! {
    #[default = std::ptr::null_mut()]
    pub static R_NilValue: SEXP;

    #[default = 0]
    pub static R_ParseError: std::ffi::c_int;

    /// 256 comes from R's `PARSE_ERROR_SIZE` define
    #[default = [0; 256usize]]
    pub static R_ParseErrorMsg: [std::ffi::c_char; 256usize];
}

mutable_globals::generate! {
    pub static mut R_interrupts_suspended: Rboolean;

    /// Special declaration for this global variable
    ///
    /// I don't fully understand this!
    ///
    /// This is exposed in Rinterface.h, which is not available on Windows:
    /// https://github.com/wch/r-source/blob/459492bc14ad5a3ff735d90a70ad71f6d5fe9faa/src/include/Rinterface.h#L176
    /// But is defined as a global variable in main.c, so presumably that is what RStudio is yanking out
    /// https://github.com/wch/r-source/blob/459492bc14ad5a3ff735d90a70ad71f6d5fe9faa/src/main/main.c#L729
    /// It controls whether R level signal handlers are set up, which presumably we don't want
    /// https://github.com/wch/r-source/blob/459492bc14ad5a3ff735d90a70ad71f6d5fe9faa/src/main/main.c#L1047
    /// RStudio sets this, and I think they access it by using this dllimport
    /// https://github.com/rstudio/rstudio/blob/07ef754fc9f27d41b100bb40d83ec3ddf485b47b/src/cpp/r/include/r/RInterface.hpp#L40
    pub static mut R_SignalHandlers: std::ffi::c_int;

    pub static mut R_DirtyImage: std::ffi::c_int;

    // -----------------------------------------------------------------------------------
    // Unix

    #[cfg(target_family = "unix")]
    pub static mut R_running_as_main_program: std::ffi::c_int;

    #[cfg(target_family = "unix")]
    pub static mut R_Interactive: Rboolean;

    #[cfg(target_family = "unix")]
    pub static mut R_InputHandlers: *const std::ffi::c_void;

    #[cfg(target_family = "unix")]
    pub static mut R_Consolefile: *mut libc::FILE;

    #[cfg(target_family = "unix")]
    pub static mut R_Outputfile: *mut libc::FILE;

    #[cfg(target_family = "unix")]
    pub static mut R_wait_usec: i32;

    #[cfg(target_family = "unix")]
    pub static mut R_PolledEvents: Option<unsafe extern "C" fn()>;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_WriteConsole: Option<
        unsafe extern "C" fn(arg1: *const std::ffi::c_char, arg2: std::ffi::c_int),
    >;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_WriteConsoleEx: Option<
        unsafe extern "C" fn(
            arg1: *const std::ffi::c_char,
            arg2: std::ffi::c_int,
            arg3: std::ffi::c_int,
        ),
    >;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_ReadConsole: Option<
        unsafe extern "C" fn(
            arg1: *const std::ffi::c_char,
            arg2: *mut std::ffi::c_uchar,
            arg3: std::ffi::c_int,
            arg4: std::ffi::c_int,
        ) -> std::ffi::c_int,
    >;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_ShowMessage: Option<unsafe extern "C" fn(arg1: *const std::ffi::c_char)>;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_Busy: Option<unsafe extern "C" fn(arg1: std::ffi::c_int)>;

    // -----------------------------------------------------------------------------------
    // Windows

    #[cfg(target_family = "windows")]
    pub static mut UserBreak: Rboolean;

    /// The codepage that R thinks it should be using for Windows.
    /// Should map to `winsafe::kernel::co::CP`.
    #[cfg(target_family = "windows")]
    pub static mut localeCP: std::ffi::c_uint;
}
