//
// startup.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;

pub unsafe fn source(file: String) {
    let mut func = RFunction::new("base", "source");
    func.param("file", file.clone());

    if let Err(error) = func.call() {
        log::error!("Failed to source startup file '{file}' due to: {error}.");
    }
}
