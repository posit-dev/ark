//
// routines.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Mutex;

use libr::R_CallMethodDef;
use libr::R_getEmbeddingDllInfo;
use libr::R_registerRoutines;
use log::error;

// Sync and Send wrapper that we can store in a global. Necessary because
// `R_CallMethodDef` includes raw pointers.
struct CallMethodDef {
    inner: R_CallMethodDef,
}

unsafe impl Send for CallMethodDef {}
unsafe impl Sync for CallMethodDef {}

static R_ROUTINES: Mutex<Vec<CallMethodDef>> = Mutex::new(vec![]);

// NOTE: This function is used via the #[harp::register] macro,
// which ensures that routines are initialized and executed on
// application startup.
pub unsafe fn add(def: R_CallMethodDef) {
    R_ROUTINES
        .lock()
        .unwrap()
        .push(CallMethodDef { inner: def });
}

pub unsafe fn r_register_routines() {
    let info = R_getEmbeddingDllInfo();
    if info.is_null() {
        error!("internal error: no embedding DllInfo available");
        return;
    }

    // Make sure we have an "empty" routine at the end.
    add(R_CallMethodDef {
        name: std::ptr::null(),
        fun: None,
        numArgs: 0,
    });

    // Now unwrap the definitions from our thread-safe type
    let unwrapped: Vec<R_CallMethodDef> = R_ROUTINES
        .lock()
        .unwrap()
        .iter()
        .map(|def| def.inner)
        .collect();

    R_registerRoutines(
        info,
        std::ptr::null(),
        unwrapped.as_ptr(),
        std::ptr::null(),
        std::ptr::null(),
    );
}
