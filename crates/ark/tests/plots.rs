use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::execute_request::ExecuteRequestPositron;
use amalthea::wire::execute_request::JupyterPositronLocation;
use amalthea::wire::execute_request::JupyterPositronPosition;
use amalthea::wire::execute_request::JupyterPositronRange;
use ark_test::DummyArkFrontend;

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

    if !frontend.is_installed("ragg") {
        report_skipped("test_ragg_is_used_by_default", "ragg");
        return;
    }

    // We install ragg on CI, but developers may not have it locally
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

    if !frontend.is_installed("ragg") {
        report_skipped("test_ragg_is_used_by_default", "ragg");
        return;
    }

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

fn report_skipped(f: &str, pkg: &str) {
    println!("Skipping `{f}()`. {pkg} is not installed.");
}

/// Test that plots receive a unique display_id that can be used for plot attribution.
///
/// This test verifies that:
/// 1. When a plot is created via execute_request, a display_data message is sent
/// 2. The display_data contains a valid display_id in the transient field
/// 3. The display_id is unique per plot
///
/// Note: Full GetMetadata RPC testing requires a Positron frontend with UI comm connected,
/// which enables dynamic plots. In the standard Jupyter frontend used by tests, plots use
/// the Jupyter protocol which sends display_data but doesn't create plot comm sockets.
#[test]
fn test_plot_has_display_id() {
    let frontend = DummyArkFrontend::lock();

    let code = "plot(1:10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Receive display data and verify it has a display_id
    let display_id = frontend.recv_iopub_display_data_id();

    // Verify the display_id is non-empty (it's a UUID-like string)
    assert!(!display_id.is_empty());

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Create a second plot and verify it gets a different display_id
    let code2 = "plot(1:5)";
    frontend.send_execute_request(code2, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input2 = frontend.recv_iopub_execute_input();
    assert_eq!(input2.code, code2);

    let display_id2 = frontend.recv_iopub_display_data_id();

    // Verify the second plot has a different display_id
    assert!(!display_id2.is_empty());
    assert_ne!(display_id, display_id2);

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input2.execution_count);
}

/// Test that plot metadata contains the correct execution_id.
///
/// This test verifies that when a plot is created, its metadata is stored
/// with the correct execution_id matching the execute_request that produced it.
/// The metadata is queried using the display_id from the display_data message.
#[test]
fn test_plot_get_metadata() {
    let frontend = DummyArkFrontend::lock();

    // Execute code that creates a plot
    let code = "plot(1:10)";
    let msg_id = frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Receive display data and get the display_id
    let display_id = frontend.recv_iopub_display_data_id();
    assert!(!display_id.is_empty());

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Query the metadata using the display_id
    let query_code = format!(".ps.graphics.get_metadata('{display_id}')");
    frontend.send_execute_request(&query_code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The result is a named list printed as:
    // $name
    // [1] "plot 1"
    //
    // $kind
    // [1] "plot"
    //
    // $execution_id
    // [1] "<msg_id>"
    //
    // $code
    // [1] "plot(1:10)"

    // Verify execution_id matches the msg_id of the execute_request
    assert!(
        result.contains(&msg_id),
        "Metadata should contain execution_id '{msg_id}', got:\n{result}"
    );

    // Verify code matches
    assert!(
        result.contains(code),
        "Metadata should contain code '{code}', got:\n{result}"
    );

    // Verify kind is "plot" for base R plots
    assert!(
        result.contains("$kind") && result.contains("\"plot\""),
        "Metadata should contain kind 'plot', got:\n{result}"
    );
}

/// Test that plot metadata includes origin when code_location is provided.
///
/// This test verifies that when an execute_request includes a `positron.code_location`,
/// the resulting plot's metadata includes the origin URI.
#[test]
fn test_plot_get_metadata_with_origin() {
    let frontend = DummyArkFrontend::lock();

    let code = "plot(1:10)";
    let origin_uri = "file:///path/to/analysis.R";

    // Send execute_request with a code_location
    frontend.send_execute_request(
        code,
        ExecuteRequestOptions {
            positron: Some(ExecuteRequestPositron {
                code_location: Some(JupyterPositronLocation {
                    uri: origin_uri.to_string(),
                    range: JupyterPositronRange {
                        start: JupyterPositronPosition {
                            line: 5,
                            character: 0,
                        },
                        end: JupyterPositronPosition {
                            line: 5,
                            character: 10,
                        },
                    },
                }),
            }),
            ..ExecuteRequestOptions::default()
        },
    );
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Receive display data and get the display_id
    let display_id = frontend.recv_iopub_display_data_id();
    assert!(!display_id.is_empty());

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Query the metadata using the display_id
    let query_code = format!(".ps.graphics.get_metadata('{display_id}')");
    frontend.send_execute_request(&query_code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Verify origin_uri is present in the metadata
    assert!(
        result.contains(origin_uri),
        "Metadata should contain origin_uri '{origin_uri}', got:\n{result}"
    );
}
