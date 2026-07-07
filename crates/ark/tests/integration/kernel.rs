use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use ark_test::DummyArkFrontend;
use stdext::assert_match;

#[test]
fn test_kernel_info() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_shell(KernelInfoRequest {});

    assert_match!(frontend.recv_shell(), Message::KernelInfoReply(reply) => {
        assert_eq!(reply.content.language_info.name, "R");
        assert_eq!(reply.content.language_info.pygments_lexer, None);
        assert_eq!(reply.content.language_info.codemirror_mode, None);
        assert_eq!(reply.content.language_info.nbconvert_exporter, None);
    });

    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

#[test]
fn test_env_vars() {
    // These environment variables are set by R's shell script frontend.
    // We set these in Ark as well.
    let frontend = DummyArkFrontend::lock();

    let code = "stopifnot(
            identical(Sys.getenv('R_SHARE_DIR'), R.home('share')),
            identical(Sys.getenv('R_INCLUDE_DIR'), R.home('include')),
            identical(Sys.getenv('R_DOC_DIR'), R.home('doc'))
        )";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

// It used to be the case that the permanent `process_console_notifications()` loop was
// spawned via an `idle_any_prompt()` task with `ConsoleOutputCapture` enabled. Because
// this loop never returned, the `warn` global option that `ConsoleOutputCapture` sets to
// `1` was never reset back to `0`, and this leaked to the user. We no longer spawn the
// `process_console_notifications()` task with capturing enabled, so this is no longer an
// issue, and this is a regression test for that scenario.
#[test]
fn test_warn_option_is_zero_on_initialization() {
    let frontend = DummyArkFrontend::lock();

    // It took quite a awhile for R to fully start up, and for the idle task that
    // `process_console_notifications()` is spawned from to get picked up by the main
    // event loop, setting the `warn` option. This is "fragile" in reproducing the
    // original issue, but now it should never fail, so we aren't worried about time based
    // fragility. If it ever fails, we have a problem!
    std::thread::sleep(std::time::Duration::from_secs(2));

    let code = "stopifnot(identical(getOption('warn'), 0L))";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}
