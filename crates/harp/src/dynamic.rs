//
// dynamic.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_snake_case)]
#![allow(non_camel_case_types)]

use std::env::var_os;
use std::ffi::OsString;

use libR_shim::Rboolean;
use libR_shim::SEXP;
use libloading::Library;
use libloading::Symbol;
use once_cell::sync::Lazy;
use semver::Version;

use crate::r_version::r_version;

/// Environment variable set by the frontend when starting ark with a specific
/// version of R. Corresponds to the absolute path to `libR.so`, `libR.dylib`,
/// or `R.dll` depending on the platform.
static ARK_R_DYNAMIC_LIBRARY_PATH: Lazy<OsString> =
    Lazy::new(|| var_os("ARK_R_DYNAMIC_LIBRARY_PATH").unwrap());

/// A global instance of the R dynamic library
///
/// We extract the symbols from this. It has a static lifetime to ensure it
/// stays alive for the lifetime of the program, ensuring our symbols also live
/// that long.
///
/// Initialized on first access.
static R_DYNAMIC_LIBRARY: Lazy<Library> = Lazy::new(|| unsafe {
    match Library::new(ARK_R_DYNAMIC_LIBRARY_PATH.as_os_str()) {
        Ok(library) => library,
        Err(err) => panic!("Failed to initialize R dynamic library: {err:?}."),
    }
});

/// A global instance of the struct that contains the dynamic function pointers
/// that we use throughout ark
///
/// Initialized on first access.
static R_DYNAMIC_SYMBOLS: Lazy<RDynamicSymbols> = Lazy::new(|| RDynamicSymbols::new());

pub struct RDynamicSymbols<'lib> {
    symbols_4_2_0: Option<RDynamicSymbols_4_2_0<'lib>>,
}

/// The public API for `RDynamicSymbols`
///
/// All symbols are exported from here
///
/// Returns `Some(value)` if the symbol exists, and `None` if it isn't available
impl<'lib> RDynamicSymbols<'lib> {
    pub fn get_R_existsVarInFrame() -> Option<&'lib Symbol<'lib, R_existsVarInFrame_t>> {
        match Self::get().symbols_4_2_0.as_ref() {
            Some(symbols) => Some(&symbols.R_existsVarInFrame),
            None => None,
        }
    }
}

impl<'lib> RDynamicSymbols<'lib> {
    /// Create an instance of `RDynamicSymbols` loaded with the symbols that
    /// are relevant for this version of R.
    fn new() -> Self {
        let version = r_version();
        const VERSION_4_2_0: Version = Version::new(4, 2, 0);

        let symbols_4_2_0 = if version >= &VERSION_4_2_0 {
            Some(RDynamicSymbols_4_2_0::new())
        } else {
            None
        };

        Self { symbols_4_2_0 }
    }

    /// Access the global instance of `RDynamicSymbols` to retrieve a symbol
    fn get() -> &'lib Self {
        if !crate::on_main_thread() {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("<unnamed>");
            let message = format!(
                "Must access `R_DYNAMIC_SYMBOLS` on the main R thread, not thread '{name}'."
            );
            #[cfg(debug_assertions)]
            panic!("{message}");
            #[cfg(not(debug_assertions))]
            log::error!("{message}");
        }

        &R_DYNAMIC_SYMBOLS
    }
}

type R_existsVarInFrame_t = unsafe extern "C" fn(SEXP, SEXP) -> Rboolean;

struct RDynamicSymbols_4_2_0<'lib> {
    R_existsVarInFrame: Symbol<'lib, R_existsVarInFrame_t>,
}

impl<'lib> RDynamicSymbols_4_2_0<'lib> {
    pub fn new() -> Self {
        unsafe {
            let R_existsVarInFrame = R_DYNAMIC_LIBRARY.get(b"R_existsVarInFrame\0").unwrap();
            Self { R_existsVarInFrame }
        }
    }
}
