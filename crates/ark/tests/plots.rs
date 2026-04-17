use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::execute_request::ExecuteRequestPositron;
use amalthea::wire::execute_request::JupyterPositronLocation;
use amalthea::wire::execute_request::JupyterPositronPosition;
use amalthea::wire::execute_request::JupyterPositronRange;
use ark_test::comm::RECV_TIMEOUT;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;
use base64::Engine;

/// Default DPI for the current OS, matching the constant in graphics_device.rs.
fn default_dpi() -> f64 {
    if cfg!(target_os = "macos") {
        96.0
    } else {
        72.0
    }
}

/// Extract pixel dimensions (width, height) from base64-encoded PNG data.
fn png_dimensions(base64_data: &str) -> (u32, u32) {
    // The base64 data may contain newlines or use non-padded encoding
    let cleaned: String = base64_data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&cleaned)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&cleaned))
        .expect("Failed to decode base64 PNG data");
    // Validate PNG signature and minimum size for IHDR
    let png_signature: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    assert!(bytes.len() >= 24);
    assert_eq!(bytes[..8], png_signature);
    // PNG IHDR: 8-byte signature, 4-byte chunk length, 4-byte "IHDR", then width (4) and height (4)
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (width, height)
}

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
    frontend.send_execute_request(code, ExecuteRequestOptions {
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
            ..Default::default()
        }),
        ..ExecuteRequestOptions::default()
    });
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

/// Test that plots are emitted when created inside source().
///
/// This test verifies that when an R file containing plot() is sourced,
/// the plot is still emitted as display_data on the IOPub channel.
#[test]
fn test_plot_from_source() {
    let frontend = DummyArkFrontend::lock();

    let file = SourceFile::new("plot(1:10)\n");

    let code = format!("source('{}')", file.path);
    frontend.send_execute_request(&code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // The sourced file creates a plot, so we should receive display_data
    frontend.recv_iopub_display_data();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that multiple plots are emitted when created inside source().
#[test]
fn test_multiple_plots_from_source() {
    let frontend = DummyArkFrontend::lock();

    let file = SourceFile::new("plot(1:10)\nplot(1:5)\nplot(1:3)\n");

    let code = format!("source('{}')", file.path);
    frontend.send_execute_request(&code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // All three plots should be emitted as display_data
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_display_data();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that plots are emitted during source() in dynamic plots mode (Positron).
///
/// When the UI comm is connected, plots use the Positron comm protocol
/// (CommOpen) instead of Jupyter's display_data. This test verifies that
/// plots created inside source() are properly emitted via comm protocol.
#[test]
fn test_plot_from_source_dynamic() {
    let frontend = DummyArkFrontend::lock();

    // Open a UI comm to enable dynamic plots (Positron mode).
    // This triggers some comm messages (prompt refresh, etc.) that we need to
    // drain before proceeding.
    frontend.open_ui_comm();

    // Test source() with a plot in dynamic mode
    let file = SourceFile::new("plot(1:10)\n");

    let code = format!("source('{}')", file.path);
    frontend.send_execute_request(&code, ExecuteRequestOptions::default());

    // In dynamic plots mode, the plot should arrive as a CommOpen.
    // The UI comm also sends CommMsg events (busy, etc.) that we need to skip.
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    let mut got_plot_comm = false;
    let mut got_idle = false;

    while !got_plot_comm || !got_idle {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(msg) = frontend.recv_iopub_with_timeout(remaining) else {
            panic!(
                "Timed out waiting for plot (got_plot_comm={got_plot_comm}, got_idle={got_idle})"
            );
        };
        match msg {
            amalthea::wire::jupyter_message::Message::CommOpen(data) => {
                assert_eq!(data.content.target_name, "positron.plot");
                got_plot_comm = true;
            },
            amalthea::wire::jupyter_message::Message::Status(data)
                if data.content.execution_state == amalthea::wire::status::ExecutionState::Idle =>
            {
                got_idle = true;
            },
            // Skip CommMsg (UI comm events), Status(Busy), ExecuteInput, Stream, etc.
            _ => {},
        }
    }
    frontend.recv_shell_execute_reply();
}

/// Test that nested source() calls attribute plots to the correct file.
///
/// When file A sources file B, and file B creates a plot, the plot's origin
/// should point to file B (the innermost source context), not file A.
/// This verifies that the source context stack correctly tracks nesting.
#[test]
fn test_plot_source_context_stacking() {
    let frontend = DummyArkFrontend::lock();

    // File B creates a plot
    let file_b = SourceFile::new("plot(1:10)\n");

    // File A sources file B, then creates its own plot
    let file_a_code = format!("source('{}')\nplot(1:5)\n", file_b.path);
    let file_a = SourceFile::new(&file_a_code);

    // Source file A from the console
    let code = format!("source('{}')", file_a.path);
    frontend.send_execute_request(&code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // First plot (from file B, sourced by file A)
    let display_id_b = frontend.recv_iopub_display_data_id();
    assert!(!display_id_b.is_empty());

    // Second plot (from file A itself)
    let display_id_a = frontend.recv_iopub_display_data_id();
    assert!(!display_id_a.is_empty());

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Query metadata for the first plot (created by file B)
    let query_b = format!(".ps.graphics.get_metadata('{display_id_b}')");
    frontend.send_execute_request(&query_b, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result_b = frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The origin_uri should point to file B, not file A
    assert!(
        result_b.contains(&file_b.uri_id),
        "Plot from file B should have origin_uri pointing to file B '{}', got:\n{result_b}",
        file_b.uri_id,
    );
    assert!(
        !result_b.contains(&file_a.uri_id),
        "Plot from file B should NOT have origin_uri pointing to file A '{}', got:\n{result_b}",
        file_a.uri_id,
    );

    // Query metadata for the second plot (created by file A)
    let query_a = format!(".ps.graphics.get_metadata('{display_id_a}')");
    frontend.send_execute_request(&query_a, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result_a = frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The origin_uri should point to file A
    assert!(
        result_a.contains(&file_a.uri_id),
        "Plot from file A should have origin_uri pointing to file A '{}', got:\n{result_a}",
        file_a.uri_id,
    );
}

/// Test that plots rendered with fig-width/fig-height metadata produce
/// a PNG at the expected pixel dimensions (inches * 96 DPI).
#[test]
fn test_plot_with_fig_size_metadata() {
    let frontend = DummyArkFrontend::lock();

    let code = "plot(1:10)";
    frontend.send_execute_request(code, ExecuteRequestOptions {
        positron: Some(ExecuteRequestPositron {
            fig_width: Some(5.0),
            fig_height: Some(4.0),
            ..Default::default()
        }),
        ..ExecuteRequestOptions::default()
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let display = frontend.recv_iopub_display_data_content();
    let png_data = display.data["image/png"]
        .as_str()
        .expect("display_data should contain image/png");
    let (width, height) = png_dimensions(png_data);

    let dpi = default_dpi();
    // 5 inches * DPI, 4 inches * DPI
    assert_eq!(width, (5.0 * dpi).round() as u32);
    assert_eq!(height, (4.0 * dpi).round() as u32);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that plots rendered with output_width_px (but no fig dimensions)
/// produce a PNG at the expected width with a 4:3 aspect ratio.
#[test]
fn test_plot_with_output_width_metadata() {
    let frontend = DummyArkFrontend::lock();

    let code = "plot(1:10)";
    frontend.send_execute_request(code, ExecuteRequestOptions {
        positron: Some(ExecuteRequestPositron {
            output_width_px: Some(600.0),
            ..Default::default()
        }),
        ..ExecuteRequestOptions::default()
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let display = frontend.recv_iopub_display_data_content();
    let png_data = display.data["image/png"]
        .as_str()
        .expect("display_data should contain image/png");
    let (width, height) = png_dimensions(png_data);

    // 600px wide, 600 / (4/3) = 450px tall
    assert_eq!(width, 600);
    assert_eq!(height, 450);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that plots without sizing metadata render at the default 800x600.
#[test]
fn test_plot_default_size_without_metadata() {
    let frontend = DummyArkFrontend::lock();

    let code = "plot(1:10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let display = frontend.recv_iopub_display_data_content();
    let png_data = display.data["image/png"]
        .as_str()
        .expect("display_data should contain image/png");
    let (width, height) = png_dimensions(png_data);

    assert_eq!(width, 800);
    assert_eq!(height, 600);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that a plot created without an active execution context (e.g. from
/// a task callback that fires between execute requests) has empty metadata.
///
/// This exercises the `capture_execution_context` fallback path where no
/// context was pushed via `graphics_on_execute_request`.
#[test]
fn test_plot_without_execution_context_has_empty_metadata() {
    let frontend = DummyArkFrontend::lock();

    // Register a task callback that creates a plot. Task callbacks fire
    // after each top-level R evaluation completes, at which point the
    // execution context has already been cleared by `on_did_execute_request`.
    // The callback removes itself after one invocation (returns FALSE).
    //
    // The task callback fires immediately after this evaluation completes
    // (but still within the busy/idle window), so the display_data for the
    // plot arrives before idle.
    frontend.send_execute_request(
        r#"invisible(addTaskCallback(function(...) { plot(1:10); FALSE }))"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let display_id = frontend.recv_iopub_display_data_id();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Query metadata using the display_id from the plot
    let query_code = format!(".ps.graphics.get_metadata('{display_id}')");
    frontend.send_execute_request(&query_code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // execution_id and code should be empty since the plot was created
    // outside of an execute request's execution context
    assert!(
        result.contains("$execution_id") && result.contains("[1] \"\""),
        "execution_id should be empty, got:\n{result}"
    );
    assert!(
        result.contains("$code") && result.contains("[1] \"\""),
        "code should be empty, got:\n{result}"
    );
}

/// Test that `dev.hold()` suppresses intermediate plot output.
///
/// Without hold, each `plot()` call emits a separate `display_data`.
/// With hold active, intermediate plots are suppressed and only the
/// final state after `dev.flush()` is emitted.
#[test]
fn test_dev_hold_suppresses_intermediate_plots() {
    let frontend = DummyArkFrontend::lock();

    // Hold, draw two intermediate plots, then flush.
    // Only the final plot should produce output.
    let code = r#"
invisible(dev.hold())
plot(1:5)
plot(1:3)
invisible(dev.flush())
"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that `dev.hold()` persists across execute requests.
///
/// A hold started in one request should suppress output until
/// `dev.flush()` is called in a subsequent request.
#[test]
fn test_dev_hold_across_execute_requests() {
    let frontend = DummyArkFrontend::lock();

    // Hold and plot without flushing. No display_data should appear.
    frontend.send_execute_request(
        "invisible(dev.hold())\nplot(1:5)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Flush in a separate request. The held plot should now appear.
    frontend.send_execute_request("invisible(dev.flush())", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_display_data();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

// Positron-path plot tests (dynamic plots via comm channels)
//
// These tests connect the UI comm to enable the Positron plot path
// (comm-based dynamic plots) instead of the Jupyter protocol path
// (display_data / update_display_data).
//
// In the Positron path:
// - New plots open a "positron.plot" comm (via CommEvent::Opened through
//   Shell, arriving on IOPub after idle).
// - Plot updates send a comm_msg directly on IOPub (arriving before idle).
//
// Regression tests for https://github.com/posit-dev/ark/pull/1100

/// Positron path: a single plot opens a plot comm.
#[test]
fn test_positron_simple_plot() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    frontend.send_execute_request("plot(1:10)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let open = frontend.recv_iopub_comm_open();
    assert_eq!(open.target_name, "positron.plot");
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: two plots in a single request each open their own comm.
#[test]
fn test_positron_multiple_plots() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    frontend.send_execute_request("plot(1:10)\nplot(2:20)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let open1 = frontend.recv_iopub_comm_open();
    let open2 = frontend.recv_iopub_comm_open();
    assert_eq!(open1.target_name, "positron.plot");
    assert_eq!(open2.target_name, "positron.plot");
    assert_ne!(open1.comm_id, open2.comm_id);
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `par(mfrow)` creates one plot comm with panel updates.
///
/// The first panel opens a plot comm; the second panel sends an update
/// on the same comm. Plot ID stays the same (no new page).
#[test]
fn test_positron_par_multi_panel() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let code = r#"
par(mfrow = c(2, 1))
plot(1:10)
id1 <- .ps.internal(current_plot_id())
plot(2:20)
id2 <- .ps.internal(current_plot_id())
stopifnot(id1 == id2)
"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let open = frontend.recv_iopub_comm_open();
    assert_eq!(open.target_name, "positron.plot");

    // Panel update arrives after comm_open (barrier ensures ordering)
    let update = frontend.recv_iopub_comm_msg();
    assert_eq!(update.comm_id, open.comm_id);

    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `layout()` + multi-plot works like `par(mfrow)`.
///
/// Same as `test_positron_par_multi_panel` but with `layout()` inside a
/// function call, which exercises a slightly different R code path.
///
/// Regression: https://github.com/posit-dev/ark/pull/1100#discussion_r2942816670
#[test]
fn test_positron_layout_multi_plot() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let code = r#"
plt2 = function() {
  layout(matrix(1:2, 2))
  plot(1, 1)
  id1 <- .ps.internal(current_plot_id())
  plot(1, 1)
  id2 <- .ps.internal(current_plot_id())
  stopifnot(id1 == id2)
}
plt2()
"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let open = frontend.recv_iopub_comm_open();
    assert_eq!(open.target_name, "positron.plot");

    // Second panel update arrives after comm_open (barrier ensures ordering)
    let update = frontend.recv_iopub_comm_msg();
    assert_eq!(update.comm_id, open.comm_id);

    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `dev.hold()` suppresses intermediate plot output.
///
/// Only the final state after `dev.flush()` produces a plot comm.
#[test]
fn test_positron_dev_hold_suppresses() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let code = r#"
invisible(dev.hold())
plot(1:5)
plot(1:3)
invisible(dev.flush())
"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Only one plot comm for the final state
    let open = frontend.recv_iopub_comm_open();
    assert_eq!(open.target_name, "positron.plot");

    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `dev.hold()` persists across execute requests.
///
/// A hold started in one request should suppress output until
/// `dev.flush()` is called in a subsequent request.
#[test]
fn test_positron_dev_hold_across_requests() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    // Hold and plot without flushing. No plot comm should open.
    frontend.send_execute_request(
        "invisible(dev.hold())\nplot(1:5)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Flush in a separate request. The held plot should now appear.
    frontend.send_execute_request("invisible(dev.flush())", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let open = frontend.recv_iopub_comm_open();
    assert_eq!(open.target_name, "positron.plot");
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: three separate requests each produce a plot comm.
///
/// Simulates running different packages (e.g. rpart, sf, rpart) one at a
/// time, each producing their own plot.
///
/// Regression: https://github.com/posit-dev/ark/pull/1100#discussion_r2942842898
#[test]
fn test_positron_sequential_plots() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    for i in 1..=3 {
        let code = format!("plot({i}:10)");
        frontend.send_execute_request(&code, ExecuteRequestOptions::default());
        frontend.recv_iopub_busy();
        frontend.recv_iopub_execute_input();
        let open = frontend.recv_iopub_comm_open();
        assert_eq!(open.target_name, "positron.plot");
        frontend.recv_iopub_ui_prompt_state();
        frontend.recv_iopub_idle();
        frontend.recv_shell_execute_reply();
    }
}

/// Positron path: switching to `png()` and back preserves our plot.
///
/// The png device is separate from the positron device and should not
/// produce plot comms.
#[test]
fn test_positron_graphics_device_swap() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let code = r#"
plot(1:10)
grDevices::png(tempfile(fileext = ".png"))
plot(1:20)
invisible(dev.off())
"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Only one plot comm for the first plot (on our device)
    let open = frontend.recv_iopub_comm_open();
    assert_eq!(open.target_name, "positron.plot");

    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: plotting in a loop produces one comm per iteration.
#[test]
fn test_positron_loop_plots() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let code = r#"
for (i in 1:3) {
  plot(i)
}
"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let open1 = frontend.recv_iopub_comm_open();
    let open2 = frontend.recv_iopub_comm_open();
    let open3 = frontend.recv_iopub_comm_open();
    assert_eq!(open1.target_name, "positron.plot");
    assert_eq!(open2.target_name, "positron.plot");
    assert_eq!(open3.target_name, "positron.plot");
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `par(mfrow)` with 4 plots in a 3-panel layout.
///
/// The first 3 plots fill the layout (1 new + 2 updates on the same comm).
/// The 4th plot overflows to a new page, opening a second comm.
#[test]
fn test_positron_par_overflow_to_new_page() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

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
    frontend.recv_iopub_execute_input();

    // First page comm (barrier ensures comm_open arrives before updates)
    let open1 = frontend.recv_iopub_comm_open();
    assert_eq!(open1.target_name, "positron.plot");

    // Panels 2 and 3 update the first page
    let update1 = frontend.recv_iopub_comm_msg();
    let update2 = frontend.recv_iopub_comm_msg();
    assert_eq!(update1.comm_id, open1.comm_id);
    assert_eq!(update2.comm_id, open1.comm_id);

    // Second page comm (4th plot overflows to a new page)
    let open2 = frontend.recv_iopub_comm_open();
    assert_eq!(open2.target_name, "positron.plot");
    assert_ne!(open1.comm_id, open2.comm_id);

    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `dev.hold()` / `dev.flush()` run one line at a time.
///
/// Each line is a separate execute request, simulating interactive use.
#[test]
fn test_positron_dev_hold_flush_interactive() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    // Hold
    frontend.send_execute_request("invisible(dev.hold())", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Draw first plot (held, no comm should open)
    frontend.send_execute_request("plot(1:10)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Draw over it (still held)
    frontend.send_execute_request("abline(1, 2)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Flush - the combined plot should now appear
    frontend.send_execute_request("invisible(dev.flush())", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let open = frontend.recv_iopub_comm_open();
    assert_eq!(open.target_name, "positron.plot");
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `comm_open` arrives while R is still executing.
///
/// Uses `readline()` as a synchronisation barrier: R blocks on stdin after
/// plotting, so receiving `comm_open` before the input request proves the
/// comm was published mid-execution (not deferred to the cleanup phase).
#[test]
fn test_positron_plot_comm_open_during_execution() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

    let code = r#"
for (i in 1:3) plot(i)
readline("sync>")
"#;
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // All 3 comm_opens arrive while R is still executing (before readline)
    let open1 = frontend.recv_iopub_comm_open();
    let open2 = frontend.recv_iopub_comm_open();
    let open3 = frontend.recv_iopub_comm_open();
    assert_eq!(open1.target_name, "positron.plot");
    assert_eq!(open2.target_name, "positron.plot");
    assert_eq!(open3.target_name, "positron.plot");

    // R is blocked on readline(), proving the comms arrived mid-execution
    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, "sync>");

    // Unblock R
    frontend.send_stdin_input_reply(String::from(""));

    // readline() return value
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Positron path: `layout()` panels share the same plot ID, overflow gets a new one.
///
/// Verifies that the second `plot()` call within a 2-panel layout doesn't
/// trigger a new page (plot ID stays the same), while a third `plot()` that
/// overflows the layout creates a new page (new plot ID).
#[test]
fn test_positron_layout_plot_id_stability() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let code = r#"
layout(matrix(1:2, 2))
plot(1, 1)
id1 <- .ps.internal(current_plot_id())
plot(1, 1)
id2 <- .ps.internal(current_plot_id())
plot(1, 1)
id3 <- .ps.internal(current_plot_id())
stopifnot(
    id1 == id2,
    id2 != id3
)
"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // First panel opens a plot comm
    let open1 = frontend.recv_iopub_comm_open();
    assert_eq!(open1.target_name, "positron.plot");

    // Second panel updates the same comm (no new page)
    let update = frontend.recv_iopub_comm_msg();
    assert_eq!(update.comm_id, open1.comm_id);

    // Third plot overflows to a new page
    let open2 = frontend.recv_iopub_comm_open();
    assert_eq!(open2.target_name, "positron.plot");
    assert_ne!(open1.comm_id, open2.comm_id);

    frontend.recv_iopub_ui_prompt_state();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
