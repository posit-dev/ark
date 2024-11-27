// repos-conf-file.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::io::Write;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontendDefaultRepos;

/// Using a configuration file, set the default CRAN repo to a custom value.
#[test]
fn test_conf_file_repos() {
    let contents = r#"# Custom CRAN repo configuration file

CRAN=https://my.cran.mirror/
Internal=https://internal.cran.mirror/
"#;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{contents}").unwrap();

    let path = file.path();
    let frontend =
        DummyArkFrontendDefaultRepos::lock(ark::repos::DefaultRepos::ConfFile(path.to_path_buf()));

    let code = r#"getOption("repos")[["CRAN"]]"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(
        frontend.recv_iopub_execute_result(),
        r#"[1] "https://my.cran.mirror/""#
    );

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}
