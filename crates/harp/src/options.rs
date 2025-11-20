//
// options.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use crate::{r_symbol, RObject};

pub fn get_option(name: &str) -> RObject {
    unsafe { libr::Rf_GetOption1(r_symbol!(name)).into() }
}

pub fn get_option_bool(name: &str) -> bool {
    harp::get_option(name).try_into().unwrap_or(false)
}
