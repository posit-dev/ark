//
// fixtures/utils.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

#[cfg(feature = "testing")]
use crate::console::Console;
use crate::modules;
use crate::modules::ARK_ENVS;

static INIT: Once = Once::new();

pub fn r_test_init() {
    harp::fixtures::r_test_init();
    INIT.call_once(|| {
        // Initialize the positron module so tests can use them.
        let ns = modules::initialize().unwrap();
        modules::initialize_options(ns.sexp).unwrap();
    });
    // Per-thread: install a minimal Console singleton so that unit tests
    // can pass `&Console` to comm handlers via `Console::get()`.
    #[cfg(feature = "testing")]
    Console::test_init();
}

pub fn package_is_installed(package: &str) -> bool {
    harp::parse_eval0(
        format!(".ps.is_installed('{package}')").as_str(),
        ARK_ENVS.positron_ns,
    )
    .unwrap()
    .try_into()
    .unwrap()
}
