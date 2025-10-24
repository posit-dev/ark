use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::recv_iopub_busy;
use amalthea::recv_iopub_display_data;
use amalthea::recv_iopub_execute_input;
use amalthea::recv_iopub_execute_result;
use amalthea::recv_iopub_idle;
use amalthea::recv_iopub_update_display_data;
use amalthea::recv_shell_execute_reply;
use ark::fixtures::DummyArkFrontend;

#[test]
fn test_basic_plot() {
    let frontend = DummyArkFrontend::lock();

    let code = "plot(1:10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_display_data!(frontend);

    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_plots_in_a_loop() {
    let frontend = DummyArkFrontend::lock();

    let code = r#"
for (i in 1:5) {
  plot(i)
}"#;

    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_display_data!(frontend);
    recv_iopub_display_data!(frontend);
    recv_iopub_display_data!(frontend);
    recv_iopub_display_data!(frontend);
    recv_iopub_display_data!(frontend);

    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
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
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_display_data!(frontend);
    recv_iopub_display_data!(frontend);

    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
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
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_display_data!(frontend);
    recv_iopub_update_display_data!(frontend);
    recv_iopub_update_display_data!(frontend);
    recv_iopub_display_data!(frontend);

    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_graphics_device_initialization() {
    let frontend = DummyArkFrontend::lock();

    // On startup we are in the interactive list, but not current device
    let code = "'.ark.graphics.device' %in% grDevices::deviceIsInteractive()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);
    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] TRUE");
    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    // The current device is `"null device"`
    let code = ".Device";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);
    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] \"null device\"");
    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    // The current `"null device"` is not interactive
    let code = "grDevices::dev.interactive()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);
    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] FALSE");
    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    // But `orNone = TRUE` looks at `options(device =)` in this case, which
    // we set to us, so this works (and is used by `demo(graphics)`)
    let code = "grDevices::dev.interactive(orNone = TRUE)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);
    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] TRUE");
    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    // Now simulate the user creating a plot, which makes us the current graphics device
    let code = "x <- .ark.graphics.device(); grDevices::dev.interactive()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);
    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] TRUE");
    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}
