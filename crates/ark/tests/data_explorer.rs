//
// data_explorer.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::event::CommManagerEvent;
use ark::data_explorer::r_data_explorer::RDataExplorer;
use ark::r_task;
use crossbeam::channel::bounded;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::test::start_r;
use harp::utils::r_envir_get;
use libr::R_GlobalEnv;

#[test]
fn test_data_explorer() {
    // Start the R interpreter
    start_r();

    // Create a dummy comm manager channel.
    let (comm_manager_tx, _comm_manager_rx) = bounded::<CommManagerEvent>(0);

    // Force the mtcars dataset to make it available. This is a sample dataset
    // that comes with R.
    r_task(|| unsafe {
        RFunction::new("base", "force")
            .param("x", "mtcars")
            .call()
            .unwrap();
        let mtcars = RObject::view(r_envir_get("mtcars", R_GlobalEnv).unwrap());
        RDataExplorer::start(String::from("test"), mtcars, comm_manager_tx).unwrap();
    });
}
