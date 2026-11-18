//
// plots_locator.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//

//! `locator()` on the Ark graphics device sends a `locate_plot_point` request
//! to the frontend and converts the reply, which is a fraction of the plot
//! image, to device coordinates (posit-dev/positron#6704).

use std::time::Duration;

use amalthea::comm::base_comm::JsonRpcReply;
use amalthea::comm::base_comm::JsonRpcResult;
use amalthea::comm::ui_comm::LocatePlotPointParams;
use amalthea::comm::ui_comm::UiFrontendRequest;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::comm_msg::CommWireMsg;
use amalthea::wire::comm_open::CommOpen;
use amalthea::wire::header::JupyterHeader;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::status::ExecutionState;
use amalthea::wire::stream::Stream;
use ark_test::comm::RECV_TIMEOUT;
use ark_test::DummyArkFrontend;
use assert_matches::assert_matches;
use serde_json::json;
use serde_json::Value;

/// Plot setup whose user coordinates equal figure fractions, with x from left
/// to right and y from bottom to top. Draws nothing, so the page is blank.
const SETUP: &str =
    "par(mar = c(0, 0, 0, 0)); plot.new(); plot.window(c(0, 1), c(0, 1), xaxs = 'i', yaxs = 'i')";

/// How long IOPub must stay quiet before we assume R sent everything it sends
/// before a stdin request. R is blocked on the request, so nothing else comes.
const QUIET: Duration = Duration::from_millis(500);

/// IOPub messages received outside the `DummyArkFrontend` buffering layer,
/// with UI comm events left out.
#[derive(Default)]
struct Received {
    messages: Vec<Message>,
    stdout: String,
    stderr: String,
}

impl Received {
    fn push(&mut self, msg: Message, ui_comm_id: &str) {
        match msg {
            Message::Stream(data) => match data.content.name {
                Stream::Stdout => self.stdout.push_str(&data.content.text),
                Stream::Stderr => self.stderr.push_str(&data.content.text),
            },
            Message::CommMsg(ref data) if data.content.comm_id == ui_comm_id => {},
            other => self.messages.push(other),
        }
    }

    fn plot_opens(&self) -> Vec<&CommOpen> {
        self.messages
            .iter()
            .filter_map(|msg| match msg {
                Message::CommOpen(data) if data.content.target_name == "positron.plot" => {
                    Some(&data.content)
                },
                _ => None,
            })
            .collect()
    }

    fn plot_updates(&self) -> Vec<&CommWireMsg> {
        self.messages
            .iter()
            .filter_map(|msg| match msg {
                Message::CommMsg(data) if data.content.data["method"] == "update" => {
                    Some(&data.content)
                },
                _ => None,
            })
            .collect()
    }

    fn execute_results(&self) -> Vec<String> {
        self.messages
            .iter()
            .filter_map(|msg| match msg {
                Message::ExecuteResult(data) => Some(
                    data.content.data["text/plain"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                ),
                _ => None,
            })
            .collect()
    }

    fn has_execute_error(&self) -> bool {
        self.messages
            .iter()
            .any(|msg| matches!(msg, Message::ExecuteError(_)))
    }
}

/// Opens the UI comm and sets the Plots pane to 400x300 CSS pixels.
fn open_ui_comm_with_pane(frontend: &DummyArkFrontend) -> String {
    let ui_comm_id = frontend.open_ui_comm();
    frontend.send_plot_render_settings(&ui_comm_id, 400, 300);
    ui_comm_id
}

/// Sends `code` with stdin allowed and receives busy and the execute input.
fn start_execute(frontend: &DummyArkFrontend, code: &str) {
    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
}

/// Receives IOPub messages until none arrives for `QUIET`.
fn recv_until_quiet(frontend: &DummyArkFrontend, ui_comm_id: &str) -> Received {
    let mut received = Received::default();
    while let Some(msg) = frontend.recv_iopub_with_timeout(QUIET) {
        received.push(msg, ui_comm_id);
    }
    received
}

/// Receives IOPub messages up to the shell idle, then the execute reply.
#[track_caller]
fn finish_execute(frontend: &DummyArkFrontend, ui_comm_id: &str) -> Received {
    let mut received = Received::default();
    loop {
        let Some(msg) = frontend.recv_iopub_with_timeout(RECV_TIMEOUT) else {
            panic!("Timeout waiting for idle");
        };
        if let Message::Status(ref data) = msg {
            if data.content.execution_state == ExecutionState::Idle {
                break;
            }
        }
        received.push(msg, ui_comm_id);
    }
    frontend.recv_shell_execute_reply();
    received
}

/// Receives the next stdin message and asserts it is a `locate_plot_point`
/// request.
#[track_caller]
fn recv_locator_request(frontend: &DummyArkFrontend) -> (JupyterHeader, LocatePlotPointParams) {
    let (header, request) = assert_matches!(
        frontend.recv_stdin(),
        Message::CommRequest(msg) => (msg.header, msg.content)
    );
    let params = assert_matches!(request, UiFrontendRequest::LocatePlotPoint(params) => params);
    (header, params)
}

fn reply_point(frontend: &DummyArkFrontend, header: JupyterHeader, x: f64, y: f64) {
    let reply = JsonRpcReply::Result(JsonRpcResult {
        result: json!({ "x": x, "y": y }),
    });
    frontend.send_stdin_with_parent(reply, header);
}

fn reply_finished(frontend: &DummyArkFrontend, header: JupyterHeader) {
    let reply = JsonRpcReply::Result(JsonRpcResult {
        result: Value::Null,
    });
    frontend.send_stdin_with_parent(reply, header);
}

#[track_caller]
fn assert_numbers(text: &str, expected: &[f64]) {
    // Round to 6 decimals, so the output of `cat()` compares equal
    let round = |x: f64| (x * 1e6).round() / 1e6;
    let actual: Vec<f64> = text
        .split_whitespace()
        .map(|word| round(word.parse().unwrap()))
        .collect();
    let expected: Vec<f64> = expected.iter().copied().map(round).collect();
    assert_eq!(actual, expected);
}

/// Clicks one point at (0.25, 0.25) of the plot image with `code` and
/// returns stdout.
fn locate_quarter_point(frontend: &DummyArkFrontend, ui_comm_id: &str, code: &str) -> String {
    start_execute(frontend, code);
    let (header, _) = recv_locator_request(frontend);
    reply_point(frontend, header, 0.25, 0.25);
    finish_execute(frontend, ui_comm_id).stdout
}

#[test]
fn test_locator_returns_clicked_point() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    let code = format!("{SETUP}; p <- locator(1); cat(p$x, p$y)");
    let stdout = locate_quarter_point(&frontend, &ui_comm_id, &code);

    // The y axis of the image points down, the user y axis points up
    assert_numbers(&stdout, &[0.25, 0.75]);
}

#[test]
fn test_locator_request_names_the_plot_comm() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    start_execute(&frontend, &format!("{SETUP}; invisible(locator(1))"));
    let (header, params) = recv_locator_request(&frontend);

    let before = recv_until_quiet(&frontend, &ui_comm_id);
    let opens = before.plot_opens();
    assert_eq!(opens.len(), 1);
    assert_eq!(params.plot_id, opens[0].comm_id);

    reply_finished(&frontend, header);
    finish_execute(&frontend, &ui_comm_id);
}

/// The image the user clicks on is rendered at the device size, so the
/// fractions in the reply map to the device. The request carries that size. The plot is drawn in an earlier
/// request, so `locator()` always sends a pre-render at `device_size()`. This
/// fails if `device_size()` doesn't invert the pane size applied to the
/// device.
#[test]
fn test_locator_prerender_matches_device_size() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);
    frontend.execute_request_collect("plot(1:10)");

    start_execute(&frontend, "invisible(locator(1))");
    let (header, params) = recv_locator_request(&frontend);

    let before = recv_until_quiet(&frontend, &ui_comm_id);
    let updates = before.plot_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(
        updates[0].data["params"]["pre_render"]["settings"]["size"],
        json!({ "width": 400, "height": 300 })
    );

    // The request names the size of that image, so the frontend takes clicks
    // only on it
    assert_eq!((params.width, params.height), (400, 300));

    reply_finished(&frontend, header);
    finish_execute(&frontend, &ui_comm_id);
}

#[test]
fn test_locator_finish_returns_points_so_far() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    start_execute(
        &frontend,
        &format!("{SETUP}; p <- locator(3); cat(length(p$x))"),
    );
    let (header, _) = recv_locator_request(&frontend);
    reply_point(&frontend, header, 0.5, 0.5);
    let (header, _) = recv_locator_request(&frontend);
    reply_finished(&frontend, header);

    let after = finish_execute(&frontend, &ui_comm_id);
    assert_numbers(&after.stdout, &[1.0]);
}

#[test]
fn test_locator_finish_first_returns_null() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    start_execute(&frontend, &format!("{SETUP}; is.null(locator(1))"));
    let (header, _) = recv_locator_request(&frontend);
    reply_finished(&frontend, header);

    let after = finish_execute(&frontend, &ui_comm_id);
    assert_eq!(after.execute_results(), vec![String::from("[1] TRUE")]);
}

/// Points drawn by `locator(type = "p")` are not in the display list until
/// `locator()` returns, because R records the whole `locator()` call when it
/// finishes. So no update is sent between clicks: it would only render the
/// page again without the new point. The points arrive with the update at the
/// end of the request.
#[test]
fn test_locator_draws_points_after_last_click() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    start_execute(
        &frontend,
        &format!("{SETUP}; invisible(locator(2, type = 'p'))"),
    );
    let (header, _) = recv_locator_request(&frontend);
    let first = recv_until_quiet(&frontend, &ui_comm_id);
    let opens = first.plot_opens();
    assert_eq!(opens.len(), 1);
    let blank_image = opens[0].data["pre_render"]["data"].clone();
    assert!(blank_image.is_string());

    reply_point(&frontend, header, 0.5, 0.5);
    let (header, _) = recv_locator_request(&frontend);
    let between = recv_until_quiet(&frontend, &ui_comm_id);
    assert_eq!(between.plot_opens().len(), 0);
    assert_eq!(between.plot_updates().len(), 0);

    reply_point(&frontend, header, 0.25, 0.25);
    let after = finish_execute(&frontend, &ui_comm_id);
    let updates = after.plot_updates();
    assert_eq!(updates.len(), 1);
    assert_ne!(updates[0].data["params"]["pre_render"]["data"], blank_image);
}

/// Drawing between two `locator()` calls in one request is sent before the
/// second click: only clicks of the same call skip the update.
#[test]
fn test_locator_sends_drawing_between_calls() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    start_execute(
        &frontend,
        &format!("{SETUP}; invisible(locator(1)); points(0.5, 0.5); invisible(locator(1))"),
    );
    let (header, _) = recv_locator_request(&frontend);
    let first = recv_until_quiet(&frontend, &ui_comm_id);
    let blank_image = first.plot_opens()[0].data["pre_render"]["data"].clone();

    reply_point(&frontend, header, 0.25, 0.25);
    let (header, _) = recv_locator_request(&frontend);
    let between = recv_until_quiet(&frontend, &ui_comm_id);
    let updates = between.plot_updates();
    assert_eq!(updates.len(), 1);
    assert_ne!(updates[0].data["params"]["pre_render"]["data"], blank_image);

    reply_finished(&frontend, header);
    finish_execute(&frontend, &ui_comm_id);
}

/// A plot drawn in the same request as `locator()` is sent once, with its
/// comm open, at the device size.
#[test]
fn test_locator_same_request_as_plot_renders_once() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    start_execute(&frontend, "plot(1:10); invisible(locator(1))");
    let (header, _) = recv_locator_request(&frontend);

    let before = recv_until_quiet(&frontend, &ui_comm_id);
    assert_eq!(before.plot_opens().len(), 1);
    assert_eq!(before.plot_updates().len(), 0);

    reply_finished(&frontend, header);
    finish_execute(&frontend, &ui_comm_id);
}

/// Interrupting while R waits for a click interrupts `locator()`. It does not
/// return normally, so the rest of the expression doesn't run.
///
/// The braces make `cat()` part of the same top-level expression as
/// `locator()`. Ark goes on with the next top-level expression of the input
/// after an interrupt (`readline()` behaves the same way), so a separate
/// `cat()` would run either way.
#[test]
fn test_locator_interrupt() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    start_execute(
        &frontend,
        &format!("{SETUP}; {{ locator(1); cat('after') }}"),
    );
    recv_locator_request(&frontend);

    let received = interrupt_and_finish(&frontend, &ui_comm_id);

    assert!(!received.stdout.contains("after"));
    assert_eq!(received.execute_results(), Vec::<String>::new());
    assert!(!received.has_execute_error());

    frontend.execute_request_with_ui_comm("1 + 1", |result| assert_eq!(result, "[1] 2"));
}

/// If the plot can't be rendered at the device size, there is no image the
/// frontend would take clicks on, so `locator()` returns at once instead of
/// waiting for a click that can't come.
#[test]
fn test_locator_returns_null_when_render_fails() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    frontend.execute_request_collect("plot(1:10)");

    // Renders replay the recording with `replayPlot()`, so this makes them
    // fail. R ignores errors inside a replayed display list, so a failing
    // `recordGraphics()` wouldn't do. If R waited for a click, this would
    // time out.
    let code = "invisible(capture.output(trace('replayPlot', quote(stop('render boom')), where = asNamespace('grDevices'), print = FALSE))); located <- locator(1); invisible(capture.output(untrace('replayPlot', where = asNamespace('grDevices')))); cat(is.null(located))";
    start_execute(&frontend, code);
    let after = finish_execute(&frontend, &ui_comm_id);
    assert_eq!(after.stdout, "TRUE");
}

/// An interrupt while R renders the page for `locator()` ends `locator()`.
/// R renders in a top-level context, so without care the render would take
/// the interrupt, and `locator()` would go on as if nothing happened.
#[test]
fn test_locator_interrupt_during_render() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);
    frontend.execute_request_collect("plot(1:10)");

    // Renders replay the recording with `replayPlot()`, so this makes them
    // slow enough to interrupt. A busy loop, like a real render, and not
    // `Sys.sleep()`: R takes interrupts during `Sys.sleep()` even when they
    // are suspended.
    frontend.execute_request_collect(
        "invisible(suppressMessages(trace('replayPlot', quote({ start <- Sys.time(); while (Sys.time() - start < 2) NULL }), where = asNamespace('grDevices'), print = FALSE)))",
    );

    start_execute(&frontend, "{ locator(1); cat('after') }");
    std::thread::sleep(Duration::from_millis(500));
    let received = interrupt_and_finish(&frontend, &ui_comm_id);

    frontend.execute_request_collect(
        "invisible(suppressMessages(untrace('replayPlot', where = asNamespace('grDevices'))))",
    );

    assert_eq!(received.stdout, "");
    assert_eq!(received.execute_results(), Vec::<String>::new());
    frontend.execute_request_with_ui_comm("1 + 1", |result| assert_eq!(result, "[1] 2"));
}

/// Interrupts the running request and receives IOPub messages up to the
/// shell and control idles, then the execute reply.
#[track_caller]
fn interrupt_and_finish(frontend: &DummyArkFrontend, ui_comm_id: &str) -> Received {
    frontend.send_interrupt_request();
    frontend.recv_control_interrupt_reply();

    // Shell idle and control busy/idle race, so receive until we have both
    // idles
    let mut received = Received::default();
    let mut shell_idle = false;
    let mut control_idle = false;
    while !(shell_idle && control_idle) {
        let Some(msg) = frontend.recv_iopub_with_timeout(RECV_TIMEOUT) else {
            panic!("Timeout waiting for idle after interrupt");
        };
        if let Message::Status(ref data) = msg {
            let parent = msg.parent_header().map(|h| h.msg_type.clone());
            match (data.content.execution_state, parent.as_deref()) {
                (ExecutionState::Idle, Some("interrupt_request")) => control_idle = true,
                (ExecutionState::Idle, Some("execute_request")) => shell_idle = true,
                _ => {},
            }
            continue;
        }
        received.push(msg, ui_comm_id);
    }
    frontend.recv_shell_execute_reply();
    received
}

/// Without a UI comm there is no frontend to click on, so `locator()`
/// returns at once.
#[test]
fn test_locator_without_ui_comm_returns_null() {
    let frontend = DummyArkFrontend::lock();

    start_execute(&frontend, "plot(1:10); is.null(locator(1))");
    let after = finish_execute(&frontend, "");
    assert_eq!(after.execute_results(), vec![String::from("[1] TRUE")]);

    assert!(!frontend.stdin_socket.poll_incoming(100).unwrap());
}

#[test]
fn test_locator_capability() {
    let frontend = DummyArkFrontend::lock();
    open_ui_comm_with_pane(&frontend);
    frontend.execute_request_collect(SETUP);

    frontend.execute_request_with_ui_comm("dev.capabilities('locator')$locator", |result| {
        assert_eq!(result, "[1] TRUE")
    });
}

/// `locator()` on a plot from an earlier request sends one sized update of
/// that plot, and nothing else.
#[test]
fn test_locator_does_not_create_plot_events_by_itself() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);
    let messages = frontend.execute_request_collect("plot(1:10)");
    let plot_comm_id = assert_matches!(
        &messages[..],
        [Message::CommOpen(data), ..] => data.content.comm_id.clone()
    );

    start_execute(&frontend, "invisible(locator(1))");
    let (header, _) = recv_locator_request(&frontend);
    let before = recv_until_quiet(&frontend, &ui_comm_id);
    reply_finished(&frontend, header);
    let after = finish_execute(&frontend, &ui_comm_id);

    assert_eq!(before.plot_opens().len() + after.plot_opens().len(), 0);
    let updates = before.plot_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].comm_id, plot_comm_id);
    assert_eq!(after.plot_updates().len(), 0);
}

#[test]
fn test_identify_uses_locator() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    let code = "par(mar = c(0, 0, 0, 0)); plot(c(0, 1), c(0, 1), xlim = c(0, 1), ylim = c(0, 1), xaxs = 'i', yaxs = 'i'); cat(identify(c(0, 1), c(0, 1), n = 1, plot = FALSE))";
    start_execute(&frontend, code);
    let (header, _) = recv_locator_request(&frontend);

    // Top right, which is the second point
    reply_point(&frontend, header, 1.0, 0.0);

    let after = finish_execute(&frontend, &ui_comm_id);
    assert_numbers(&after.stdout, &[2.0]);
}

/// With the default `plot = TRUE`, `identify()` labels the point it found.
/// The label is in the display list only after `identify()` returns, so it
/// arrives with the update at the end of the request, as the points of
/// `locator(type = "p")` do.
#[test]
fn test_identify_labels_point_after_it_returns() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    // The points are away from the edges, so the labels aren't clipped
    let code = "par(mar = c(0, 0, 0, 0)); plot(c(0.25, 0.75), c(0.25, 0.75), xlim = c(0, 1), ylim = c(0, 1), xaxs = 'i', yaxs = 'i'); cat(identify(c(0.25, 0.75), c(0.25, 0.75), labels = c('first', 'second'), n = 1))";
    start_execute(&frontend, code);
    let (header, _) = recv_locator_request(&frontend);
    let before = recv_until_quiet(&frontend, &ui_comm_id);
    let opens = before.plot_opens();
    assert_eq!(opens.len(), 1);
    let unlabelled_image = opens[0].data["pre_render"]["data"].clone();
    assert!(unlabelled_image.is_string());

    // The second point
    reply_point(&frontend, header, 0.75, 0.25);

    let after = finish_execute(&frontend, &ui_comm_id);
    assert_numbers(&after.stdout, &[2.0]);
    let updates = after.plot_updates();
    assert_eq!(updates.len(), 1);
    assert_ne!(
        updates[0].data["params"]["pre_render"]["data"],
        unlabelled_image
    );
}

/// The usual way to use `identify()` is to click some points and then
/// finish. The labels are still sent after `identify()` returns.
#[test]
fn test_identify_labels_points_after_finish() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    let code = "par(mar = c(0, 0, 0, 0)); plot(c(0.25, 0.75), c(0.25, 0.75), xlim = c(0, 1), ylim = c(0, 1), xaxs = 'i', yaxs = 'i'); cat(identify(c(0.25, 0.75), c(0.25, 0.75), labels = c('first', 'second')))";
    start_execute(&frontend, code);
    let (header, _) = recv_locator_request(&frontend);
    let before = recv_until_quiet(&frontend, &ui_comm_id);
    let unlabelled_image = before.plot_opens()[0].data["pre_render"]["data"].clone();
    assert!(unlabelled_image.is_string());

    // The second point, then Finish
    reply_point(&frontend, header, 0.75, 0.25);
    let (header, _) = recv_locator_request(&frontend);
    reply_finished(&frontend, header);

    let after = finish_execute(&frontend, &ui_comm_id);
    assert_numbers(&after.stdout, &[2.0]);
    let updates = after.plot_updates();
    assert_eq!(updates.len(), 1);
    assert_ne!(
        updates[0].data["params"]["pre_render"]["data"],
        unlabelled_image
    );
}

#[test]
fn test_grid_locator() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    let code =
        "grid::grid.newpage(); p <- grid::grid.locator('npc'); cat(as.numeric(p$x), as.numeric(p$y))";
    let stdout = locate_quarter_point(&frontend, &ui_comm_id, code);

    assert_numbers(&stdout, &[0.25, 0.75]);
}

/// The png device's y axis points down (`bottom > top`), unlike ragg's, so
/// this checks the coordinate conversion for the other y direction.
#[test]
fn test_locator_with_png_fallback_device() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = frontend.open_ui_comm();
    frontend.execute_request_with_ui_comm(
        "options(ark.ragg = FALSE); .ps.internal(use_ragg())",
        |result| assert_eq!(result, "[1] FALSE"),
    );
    frontend.execute_request_with_ui_comm("x <- .ark.graphics.device(); 'ok'", |result| {
        assert_eq!(result, "[1] \"ok\"")
    });
    frontend.send_plot_render_settings(&ui_comm_id, 400, 300);

    let code = format!("{SETUP}; p <- locator(1); cat(p$x, p$y)");
    let stdout = locate_quarter_point(&frontend, &ui_comm_id, &code);

    assert_numbers(&stdout, &[0.25, 0.75]);
}

/// A replayed display list can call `locator()` (`recordGraphics()`). The
/// replay runs while the frontend waits for a resize to finish, so it must not
/// ask for a point: `locator()` returns `NULL` at once.
#[test]
fn test_resize_replay_does_not_locate() {
    let frontend = DummyArkFrontend::lock();
    let ui_comm_id = open_ui_comm_with_pane(&frontend);

    // Calls `locator()` only on replay, and records what it returned
    frontend.execute_request_collect(
        "first <- TRUE; replayed <- 'no'; plot.new(); recordGraphics({ if (!first) replayed <<- if (is.null(locator(1))) 'null' else 'point'; first <<- FALSE }, list(), environment())",
    );

    // Without the guard, R would wait for a stdin reply and this would time out
    // waiting for idle
    frontend.send_plot_render_settings(&ui_comm_id, 900, 450);

    frontend.execute_request_with_ui_comm("replayed", |result| assert_eq!(result, "[1] \"null\""));
}
