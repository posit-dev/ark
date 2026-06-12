use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontendNotebook;

/// R helper that creates a fake widget-shaped value with a single
/// `htmlDependency` pointing at a tempfile JS asset, then assigns it to the
/// global `widget` variable. The dep is named "fakedep@1.0" so tests can
/// reason about a stable cache key. The JS file contents embed `js_marker`
/// so tests can tell deps apart in the emitted payload.
fn setup_fake_widget(js_marker: &str) -> String {
    format!(
        r#"
js_file <- tempfile(fileext = ".js")
writeLines("/* {js_marker} */", js_file)
fake_dep <- htmltools::htmlDependency(
    name = "fakedep",
    version = "1.0",
    src = c(file = dirname(js_file)),
    script = basename(js_file)
)
widget <- htmltools::attachDependencies(
    htmltools::tagList(htmltools::div("widget body", id = "fake-widget")),
    list(fake_dep)
)
"#
    )
}

/// Send R code, then receive busy / input / display_data, returning the
/// `text/html` payload from the display_data message. Finishes the IOPub
/// cycle (idle + execute_reply) before returning so a following request in
/// the same test starts cleanly.
fn execute_and_get_html(frontend: &DummyArkFrontendNotebook, code: &str) -> String {
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let _ = frontend.recv_iopub_execute_input();
    let content = frontend.recv_iopub_display_data_content();
    frontend.recv_iopub_idle();
    let _ = frontend.recv_shell_execute_reply();
    content.data["text/html"]
        .as_str()
        .expect("text/html payload should be a string")
        .to_string()
}

#[test]
fn test_html_widget_emits_self_contained_html() {
    let frontend = DummyArkFrontendNotebook::lock();

    let mut code = setup_fake_widget("marker-A");
    code.push_str(".ps.view_html_widget(widget)\n");

    let html = execute_and_get_html(&frontend, &code);

    assert!(html.contains("<!DOCTYPE html>"));
    // Dependency JS is inlined literally into a `<script>` block, not referenced
    // as a base64 `data:` URI (which some notebook renderers load asynchronously,
    // breaking load order and the AMD guard).
    assert!(html.contains("/* marker-A */"));
    assert!(!html.contains("data:application/javascript;base64,"));
    assert!(html.contains("widget body"));

    // No raw tempdir references — the whole point of self-containment is that
    // the saved notebook stays valid after the temp directory is gone.
    let tempdir = std::env::temp_dir();
    let tempdir_str = tempdir.to_string_lossy();
    assert!(!html.contains(tempdir_str.as_ref()));
    assert!(!html.contains("<script src=\"lib/"));
}

#[test]
fn test_html_widget_does_not_dedupe_by_default() {
    // Positron's notebook view isolates each cell's output, so dedup would
    // strand later widgets without access to the JS loaded in earlier cells.
    // The safe default is to re-inline deps every time.
    let frontend = DummyArkFrontendNotebook::lock();

    let mut first = setup_fake_widget("marker-first");
    first.push_str(".ps.view_html_widget(widget)\n");

    let mut second = setup_fake_widget("marker-second");
    second.push_str(".ps.view_html_widget(widget)\n");

    let first_html = execute_and_get_html(&frontend, &first);
    let second_html = execute_and_get_html(&frontend, &second);

    assert!(first_html.contains("/* marker-first */"));
    assert!(second_html.contains("/* marker-second */"));
}

#[test]
fn test_html_widget_dedupe_can_be_enabled() {
    // Power users on frontends with a shared output DOM (classic Jupyter,
    // JupyterLab) can opt into IRkernel-style dedup to keep notebooks small.
    let frontend = DummyArkFrontendNotebook::lock();

    let mut first = String::from("options(ark.html_widget.deduplicate = TRUE)\n");
    first.push_str(&setup_fake_widget("marker-first"));
    first.push_str(".ps.view_html_widget(widget)\n");

    // The option and the per-session dep cache both persist across cells in
    // a session, so the second cell sees `fakedep@1.0` as already inlined.
    let mut second = setup_fake_widget("marker-second");
    second.push_str(".ps.view_html_widget(widget)\n");

    let first_html = execute_and_get_html(&frontend, &first);
    let second_html = execute_and_get_html(&frontend, &second);

    // First cell inlines the dep.
    assert!(first_html.contains("/* marker-first */"));
    // Second cell sees `fakedep@1.0` as already inlined and skips it.
    assert!(!second_html.contains("/* marker-second */"));
    // Widget body still rendered both times.
    assert!(second_html.contains("widget body"));
}

#[test]
fn test_html_widget_auto_print_emits_only_display_data() {
    // Regression for the print-override path. In real usage a user evaluates
    // `plot_ly(...)` as the last expression of a cell, R auto-prints it,
    // dispatch lands on our `print.htmlwidget` override, and a single
    // `display_data` is emitted. Crucially, because the override doesn't
    // write to stdout, `console_repl::prepare_execute_reply` should
    // accumulate no autoprint output and therefore emit no `execute_result`
    // — otherwise the notebook would render both the widget *and* a stray
    // `<htmlwidget>` text line.
    //
    // `execute_and_get_html` ends with `recv_iopub_idle()` which panics on
    // any unexpected IOPub message between `display_data` and `idle`, so a
    // regression that adds a stray `execute_result` here fails loudly.
    let frontend = DummyArkFrontendNotebook::lock();

    let mut code = setup_fake_widget("marker-autoprint");
    // Tests don't load `htmlwidgets`, so the package-load hook hasn't run
    // and the S3 override isn't installed. Install it manually and tag the
    // fake widget so `print(widget)` dispatches to `print.htmlwidget`.
    code.push_str("class(widget) <- c('htmlwidget', class(widget))\n");
    code.push_str(".ps.viewer.addOverrides()\n");
    // Bare `widget` is the cell's last visible expression, triggering R's
    // auto-print machinery — the path this regression test cares about.
    code.push_str("widget\n");

    let html = execute_and_get_html(&frontend, &code);

    assert!(html.contains("widget body"));
    assert!(html.contains("/* marker-autoprint */"));
}
