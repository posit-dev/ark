#[cfg(unix)]
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Status;
use ark_test::DummyArkFrontend;

/// Install a SIGINT handler for shutdown tests. This overrides the test runner
/// handler so it doesn't cancel our test.
#[cfg(unix)]
fn install_sigint_handler() {
    extern "C" fn sigint_handler(_: libc::c_int) {}
    unsafe {
        use nix::sys::signal::signal;
        use nix::sys::signal::SigHandler;
        use nix::sys::signal::Signal;

        signal(Signal::SIGINT, SigHandler::Handler(sigint_handler)).unwrap();
    }
}

// Note that because of these shutdown tests you _have_ to use `cargo nextest`
// instead of `cargo test`, so that each test has its own process and R thread.
#[test]
#[cfg(unix)]
fn test_shutdown_request() {
    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    frontend.send_shutdown_request(false);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, false);

    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

#[test]
#[cfg(unix)]
fn test_shutdown_request_with_restart() {
    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    frontend.send_shutdown_request(true);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, true);

    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

static SHUTDOWN_TESTS_ENABLED: bool = false;

// Can shut down Ark when running a nested debug console
// https://github.com/posit-dev/positron/issues/6553
#[test]
#[cfg(unix)]
fn test_shutdown_request_browser() {
    if !SHUTDOWN_TESTS_ENABLED {
        return;
    }

    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    // browser() at top level enters debug mode without visible output
    // ("Called from:" is filtered from console output)
    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.send_shutdown_request(true);
    frontend.recv_iopub_busy();

    // There is a race condition between the Control thread and the Shell
    // threads. Ideally we'd wait for both the Shutdown reply and the IOPub Idle
    // messages concurrently instead of sequentially.
    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, true);

    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

#[test]
#[cfg(unix)]
fn test_shutdown_request_while_busy() {
    if !SHUTDOWN_TESTS_ENABLED {
        return;
    }

    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    let code = "Sys.sleep(10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.send_shutdown_request(false);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, false);

    // Drain any streams from the interrupted Sys.sleep execution. The stream
    // could arrive before or after the shutdown idle (race condition), so we
    // drain here to prevent `recv_iopub_idle` from panicking if it arrives early.
    frontend.drain_streams();

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}
