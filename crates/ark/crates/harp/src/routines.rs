//
// routines.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use libr::R_CallMethodDef;
use libr::R_getEmbeddingDllInfo;
use libr::R_registerRoutines;
use log::error;

static mut R_ROUTINES: Vec<R_CallMethodDef> = vec![];

// NOTE: This function is used via the #[harp::register] macro,
// which ensures that routines are initialized and executed on
// application startup.
pub unsafe fn add(def: R_CallMethodDef) {
    R_ROUTINES.push(def);
}

pub unsafe fn r_register_routines() {
    let info = R_getEmbeddingDllInfo();
    if info.is_null() {
        error!("internal error: no embedding DllInfo available");
        return;
    }

    // Make sure we have an "empty" routine at the end.
    R_ROUTINES.push(R_CallMethodDef {
        name: std::ptr::null(),
        fun: None,
        numArgs: 0,
    });

    R_registerRoutines(
        info,
        std::ptr::null(),
        R_ROUTINES.as_ptr(),
        std::ptr::null(),
        std::ptr::null(),
    );
}
