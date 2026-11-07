use amalthea::comm::base_comm::JsonRpcReply;
use amalthea::comm::base_comm::JsonRpcResult;
use amalthea::comm::ui_comm::ExecuteCodeInNewSessionParams;
use amalthea::comm::ui_comm::UiFrontendRequest;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Message;
use ark_test::DummyArkFrontend;
use assert_matches::assert_matches;
use serde_json::Value;

#[test]
fn test_get_version() {
    let frontend = DummyArkFrontend::lock();

    if !has_rstudioapi(&frontend) {
        report_skipped("test_get_version");
        return;
    }

    let value = "1.0.0";
    // Can't directly talk to R, need an `r_task()` that can be used alongside
    // the `frontend`. See https://github.com/posit-dev/ark/issues/609.
    // harp::envvar::set_var("POSITRON_VERSION", value);
    set_var("POSITRON_VERSION", value, &frontend);

    let code = "as.character(rstudioapi::getVersion())";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(
        frontend.recv_iopub_execute_result(),
        format!("[1] \"{value}\"")
    );

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

#[test]
fn test_get_mode() {
    let frontend = DummyArkFrontend::lock();

    if !has_rstudioapi(&frontend) {
        report_skipped("test_get_mode");
        return;
    }

    let value = "desktop";
    // Can't directly talk to R, need an `r_task()` that can be used alongside
    // the `frontend`. See https://github.com/posit-dev/ark/issues/609.
    // harp::envvar::set_var("POSITRON_MODE", value);
    set_var("POSITRON_MODE", value, &frontend);

    let code = "rstudioapi::getMode()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(
        frontend.recv_iopub_execute_result(),
        format!("[1] \"{value}\"")
    );

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

fn set_var(key: &str, value: &str, frontend: &DummyArkFrontend) {
    let code = format!("Sys.setenv({key} = \"{value}\")");
    frontend.send_execute_request(code.as_str(), ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

fn has_rstudioapi(frontend: &DummyArkFrontend) -> bool {
    frontend.is_installed("rstudioapi")
}

fn report_skipped(f: &str) {
    println!("Skipping `{f}()`. rstudioapi is not installed.");
}

/// Mirrors R's `deparse()` of a string: double-quoted, with `"` and `\\`
/// escaped. Single quotes are left alone.
fn deparse_str(x: &str) -> String {
    format!("\"{}\"", x.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Executes `code` (which must call `rstudioapi::jobRunScript()`), asserts the
/// `execute_code_in_new_session` request, replies with a session ID, and
/// finishes the execution. Returns the request params.
fn run_job(
    frontend: &DummyArkFrontend,
    comm_id: &str,
    code: &str,
) -> ExecuteCodeInNewSessionParams {
    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let request = assert_matches!(frontend.recv_stdin(), Message::CommRequest(msg) => msg.content);
    let params = assert_matches!(
        request,
        UiFrontendRequest::ExecuteCodeInNewSession(params) => params
    );

    frontend.send_stdin(JsonRpcReply::Result(JsonRpcResult {
        result: Value::from("job-1"),
    }));

    // The result is invisible, so there is no execute result
    frontend.recv_ui_prompt_state(comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    params
}

/// Executes `code` and asserts that it errors with `needle` without sending a
/// comm request.
fn run_job_error(frontend: &DummyArkFrontend, comm_id: &str, code: &str, needle: &str) {
    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert!(frontend.recv_iopub_execute_error().contains(needle));
    frontend.recv_ui_prompt_state(comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();
}

/// Absolute path with forward slashes, as R's `normalizePath(winslash = "/")`
/// returns it. On Windows `canonicalize()` yields verbatim paths (`\\?\C:\...`,
/// `\\?\UNC\server\...`), which R does not, so strip that prefix.
fn normalized(path: &std::path::Path) -> String {
    let path = path.canonicalize().unwrap().to_string_lossy().to_string();
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_string()
    };
    path.replace('\\', "/")
}

fn write_script(dir: &std::path::Path, name: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, "1\n").unwrap();
    normalized(&path)
}

#[test]
fn test_job_run_script_sources_in_new_session() {
    let frontend = DummyArkFrontend::lock();
    if !has_rstudioapi(&frontend) {
        report_skipped("test_job_run_script_sources_in_new_session");
        return;
    }

    let comm_id = frontend.open_ui_comm();

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("with space's");
    std::fs::create_dir(&dir).unwrap();
    let path = write_script(&dir, "job.R");
    let dir = normalized(&dir);

    let code = format!("rstudioapi::jobRunScript({})", deparse_str(&path));
    let params = run_job(&frontend, &comm_id, &code);

    assert_eq!(
        UiFrontendRequest::ExecuteCodeInNewSession(params),
        UiFrontendRequest::ExecuteCodeInNewSession(ExecuteCodeInNewSessionParams {
            language_id: String::from("r"),
            code: format!(
                "{{\n    setwd({})\n    source({}, encoding = \"unknown\")\n}}",
                deparse_str(&dir),
                deparse_str(&path)
            ),
            session_name: String::from("job.R"),
        })
    );
}

#[test]
fn test_job_run_script_uses_name_and_working_dir() {
    let frontend = DummyArkFrontend::lock();
    if !has_rstudioapi(&frontend) {
        report_skipped("test_job_run_script_uses_name_and_working_dir");
        return;
    }

    let comm_id = frontend.open_ui_comm();

    let tmp = tempfile::tempdir().unwrap();
    let path = write_script(tmp.path(), "job.R");
    let other = tempfile::tempdir().unwrap();
    let other_dir = normalized(other.path());

    let code = format!(
        "rstudioapi::jobRunScript({}, name = \"targets\", workingDir = {})",
        deparse_str(&path),
        deparse_str(&other_dir)
    );
    let params = run_job(&frontend, &comm_id, &code);

    assert_eq!(params.session_name, "targets");
    assert!(params
        .code
        .starts_with(&format!("{{\n    setwd({})", deparse_str(&other_dir))));
}

#[test]
fn test_job_run_script_relative_paths_use_callers_wd() {
    let frontend = DummyArkFrontend::lock();
    if !has_rstudioapi(&frontend) {
        report_skipped("test_job_run_script_relative_paths_use_callers_wd");
        return;
    }

    let comm_id = frontend.open_ui_comm();

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    let root = write_script(tmp.path(), "a.R");
    let root = root.strip_suffix("/a.R").unwrap().to_string();

    // Restore the working directory so later tests are unaffected
    let code = format!(
        "local({{
            old <- setwd({})
            on.exit(setwd(old))
            rstudioapi::jobRunScript('a.R', workingDir = 'sub')
        }})",
        deparse_str(&root)
    );
    let params = run_job(&frontend, &comm_id, &code);

    assert_eq!(
        params.code,
        format!(
            "{{\n    setwd({})\n    source({}, encoding = \"unknown\")\n}}",
            deparse_str(&format!("{root}/sub")),
            deparse_str(&format!("{root}/a.R"))
        )
    );
}

#[test]
fn test_job_run_script_missing_working_dir_errors() {
    let frontend = DummyArkFrontend::lock();
    if !has_rstudioapi(&frontend) {
        report_skipped("test_job_run_script_missing_working_dir_errors");
        return;
    }

    let comm_id = frontend.open_ui_comm();

    let tmp = tempfile::tempdir().unwrap();
    let path = write_script(tmp.path(), "job.R");
    let missing = format!("{}/nope", path.strip_suffix("/job.R").unwrap());

    let code = format!(
        "rstudioapi::jobRunScript({}, workingDir = {})",
        deparse_str(&path),
        deparse_str(&missing)
    );
    run_job_error(&frontend, &comm_id, &code, "working directory");
}

#[test]
fn test_job_run_script_missing_file_errors() {
    let frontend = DummyArkFrontend::lock();
    if !has_rstudioapi(&frontend) {
        report_skipped("test_job_run_script_missing_file_errors");
        return;
    }

    let comm_id = frontend.open_ui_comm();

    run_job_error(
        &frontend,
        &comm_id,
        "rstudioapi::jobRunScript('does-not-exist.R')",
        "does-not-exist.R",
    );
}

#[test]
fn test_run_script_job_shim_missing_file_errors() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // Calls the shim directly, so rstudioapi's own wrapper cannot fail first.
    // A comm request would break the error flow `run_job_error` expects.
    run_job_error(
        &frontend,
        &comm_id,
        ".rs.api.runScriptJob('does-not-exist.R')",
        "does not exist",
    );
}

#[test]
fn test_run_script_job_shim_rejects_non_scalar_inputs() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // Each call fails validation before any comm request is sent.
    for (code, needle) in [
        (".rs.api.runScriptJob(c('a.R', 'b.R'))", "`path`"),
        (".rs.api.runScriptJob(NA)", "`path`"),
        (".rs.api.runScriptJob('a.R', name = 1)", "`name`"),
    ] {
        run_job_error(&frontend, &comm_id, code, needle);
    }
}

#[test]
fn test_execute_code_in_new_session_unparseable_code_errors() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // A comm request would break the error flow `run_job_error` expects.
    run_job_error(
        &frontend,
        &comm_id,
        ".ps.ui.executeCodeInNewSession('2 +', 'x')",
        "does not parse",
    );
}

#[test]
fn test_job_run_script_warns_on_import_env() {
    let frontend = DummyArkFrontend::lock();
    if !has_rstudioapi(&frontend) {
        report_skipped("test_job_run_script_warns_on_import_env");
        return;
    }

    let comm_id = frontend.open_ui_comm();

    let tmp = tempfile::tempdir().unwrap();
    let path = write_script(tmp.path(), "job.R");

    let code = format!(
        "rstudioapi::jobRunScript({}, importEnv = TRUE)",
        deparse_str(&path)
    );
    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    frontend.send_execute_request(&code, options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    assert_matches!(frontend.recv_stdin(), Message::CommRequest(msg) => {
        assert_matches!(msg.content, UiFrontendRequest::ExecuteCodeInNewSession(_));
    });
    frontend.send_stdin(JsonRpcReply::Result(JsonRpcResult {
        result: Value::from("job-1"),
    }));

    frontend.assert_stream_stderr_contains("importEnv");
    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_run_script_job_shim_empty_name_falls_back_to_file_name() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    let tmp = tempfile::tempdir().unwrap();
    let path = write_script(tmp.path(), "job.R");

    let code = format!(".rs.api.runScriptJob({}, name = \"\")", deparse_str(&path));
    let params = run_job(&frontend, &comm_id, &code);
    assert_eq!(params.session_name, "job.R");
}

#[test]
fn test_run_script_job_shim_accepts_vector_export_env() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    let tmp = tempfile::tempdir().unwrap();
    let path = write_script(tmp.path(), "job.R");

    let code = format!(
        ".rs.api.runScriptJob({}, exportEnv = c(\"a\", \"b\"))",
        deparse_str(&path)
    );
    run_job(&frontend, &comm_id, &code);
}
