//
// test.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Wrapper around `harp::r_test_impl()` that also initializes the ark level R
// modules, so they can be utilized in the tests

use std::sync::Once;

use harp::routines::r_register_routines;

use crate::modules;

pub fn r_test<F: FnOnce()>(f: F) {
    let f = || {
        initialize_ark();
        f()
    };
    harp::test::r_test(f)
}

static INIT: Once = Once::new();

fn initialize_ark() {
    INIT.call_once(|| unsafe {
        // Register routines so they are callable from the modules
        r_register_routines();

        // Initialize the public/private R function modules so tests can use them.
        modules::initialize(true).unwrap();
    });
}
