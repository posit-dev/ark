//
// test.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Wrapper around `harp::r_test_impl()` that also initializes the ark level R
// modules, so they can be utilized in the tests

use std::sync::Once;

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
    INIT.call_once(|| {
        // Initialize the positron module so tests can use them.
        // Routines are already registered by `harp::test::r_test()`.
        modules::initialize(true).unwrap();
    });
}
