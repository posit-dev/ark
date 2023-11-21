/*
 * interface.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::os::raw::c_char;

pub fn setup_r(mut _args: Vec<*mut c_char>) {
    // TODO: Windows
}

pub fn run_r() {
    // TODO: Windows
}

pub fn run_activity_handlers() {
    // Nothing to do on Windows
}
