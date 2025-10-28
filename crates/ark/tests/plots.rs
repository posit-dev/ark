use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontend;

#[test]
fn test_basic_plot() {
    let frontend = DummyArkFrontend::lock();

    let code = "plot(1:10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_display_data();

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_plots_in_a_loop() {
    let frontend = DummyArkFrontend::lock();

    let code = r#"
for (i in 1:5) {
  plot(i)
}"#;

    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_display_data();
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_display_data();

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_plot_with_graphics_device_swap() {
    let frontend = DummyArkFrontend::lock();

    let code = r#"
plot(1:5)

# Swap devices, "deactivating" our ark graphics device
# Specify a tempfile() so we can clean it up later
temp_file <- tempfile(fileext = ".png")
grDevices::png(temp_file)

# Turn the png device back off, "activating" our ark graphics device
# Returns the new current device
capture <- dev.off()

plot(6:10)

# Clean up the temporary file and suppress any output
if (file.exists(temp_file)) {
    invisible(file.remove(temp_file))
}
"#;

    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_display_data();
    frontend.recv_iopub_display_data();

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_plot_with_par_and_plot_updates() {
    let frontend = DummyArkFrontend::lock();

    let code = r#"
par(mfrow = c(3, 1))

plot(1:3)
plot(4:6)
plot(7:9)
plot(10:12)

par(mfrow = c(1, 1))
"#;

    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_display_data();
    frontend.recv_iopub_update_display_data();
    frontend.recv_iopub_update_display_data();
    frontend.recv_iopub_display_data();

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_graphics_device_initialization() {
    let frontend = DummyArkFrontend::lock();

    // On startup we are in the interactive list, but not current device
    let code = "'.ark.graphics.device' %in% grDevices::deviceIsInteractive()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] TRUE");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // The current device is `"null device"`
    let code = ".Device";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"null device\"");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // The current `"null device"` is not interactive
    let code = "grDevices::dev.interactive()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] FALSE");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // But `orNone = TRUE` looks at `options(device =)` in this case, which
    // we set to us, so this works (and is used by `demo(graphics)`)
    let code = "grDevices::dev.interactive(orNone = TRUE)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] TRUE");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Now simulate the user creating a plot, which makes us the current graphics device
    let code = "x <- .ark.graphics.device(); grDevices::dev.interactive()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] TRUE");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_ragg_is_used_by_default() {
    let frontend = DummyArkFrontend::lock();

    // We install ragg on CI and expect developers to have it locally
    let code = ".ps.internal(use_ragg())";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] TRUE");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_inability_to_load_ragg_falls_back_to_base_graphics() {
    // https://github.com/posit-dev/ark/issues/917
    let frontend = DummyArkFrontend::lock();

    // Mock `loadNamespace()` with a version that will fail on ragg
    let code = r#"
oldLoadNamespace <- base::loadNamespace
unlockBinding("loadNamespace", .BaseNamespaceEnv)

newLoadNamespace <- function(package, ...) {
  if (identical(package, "ragg")) {
    stop("Can't load ragg")
  }
  oldLoadNamespace(package, ...)
}

assign("loadNamespace", newLoadNamespace, envir = .BaseNamespaceEnv)
lockBinding("loadNamespace", .BaseNamespaceEnv)
    "#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // ragg is installed on CI, so our graphics code should try and use it,
    // but should fail when loading the package and should fall back to base R graphics
    let code = ".ps.internal(use_ragg())";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] FALSE");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "plot(1:10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}
