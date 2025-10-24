//
// repos-auto.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::io::Write;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::recv_iopub_busy;
use amalthea::recv_iopub_execute_input;
use amalthea::recv_iopub_execute_result;
use amalthea::recv_iopub_idle;
use amalthea::recv_shell_execute_reply;
use ark::fixtures::DummyArkFrontendDefaultRepos;

/// Using the automatic repos setting, the default CRAN repo should be set to the global RStudio
/// CRAN mirror.
#[test]
fn test_auto_repos() {
    // Use a startup file to force a standardized `repos` on startup,
    // regardless of what your local R version has set (i.e. from rig)
    let contents = r#"options(repos = c(CRAN = "@CRAN@"))"#;
    let mut startup_file = tempfile::NamedTempFile::new().unwrap();
    write!(startup_file, "{contents}").unwrap();
    let startup_path = startup_file.path();

    let frontend = DummyArkFrontendDefaultRepos::lock(
        ark::repos::DefaultRepos::Auto,
        startup_path.to_str().unwrap().to_string(),
    );

    let code = r#"getOption("repos")[["CRAN"]]"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(
        recv_iopub_execute_result!(frontend),
        r#"[1] "https://cran.rstudio.com/""#
    );

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count)
}
