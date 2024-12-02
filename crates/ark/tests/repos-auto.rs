//
// repos-auto.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontendDefaultRepos;

/// Using the automatic repos setting, the default CRAN repo should be set to the global RStudio
/// CRAN mirror.
#[test]
fn test_auto_repos() {
    let frontend = DummyArkFrontendDefaultRepos::lock(ark::repos::DefaultRepos::Auto);

    let code = r#"getOption("repos")[["CRAN"]]"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(
        frontend.recv_iopub_execute_result(),
        r#"[1] "https://cran.rstudio.com/""#
    );

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}
