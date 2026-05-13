use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontendNotebook;

/// R helper that creates a fake `htmlwidget` value with a single
/// `htmlDependency` pointing at a tempfile JS asset, then assigns it to the
/// global `widget` variable. The dep is named "fakedep@1.0" so tests can
/// reason about a stable cache key. The JS content (`<<JS>>`) is a unique
/// marker per cell so we can tell deps apart in the emitted payload.
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
/// `text/html` payload from the display_data message. Test finishes the
/// IOPub cycle (idle + execute_reply) before returning so the next test
/// starts cleanly.
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

    let mut code = String::from(".ps.html_widget_reset_deps()\n");
    code.push_str(&setup_fake_widget("marker-A"));
    code.push_str(".ps.view_html_widget(widget)\n");

    let html = execute_and_get_html(&frontend, &code);

    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("data:application/javascript;base64,"));
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

    let mut first = String::from(".ps.html_widget_reset_deps()\n");
    first.push_str(&setup_fake_widget("marker-first"));
    first.push_str(".ps.view_html_widget(widget)\n");

    let mut second = String::new();
    second.push_str(&setup_fake_widget("marker-second"));
    second.push_str(".ps.view_html_widget(widget)\n");

    let first_html = execute_and_get_html(&frontend, &first);
    let second_html = execute_and_get_html(&frontend, &second);

    assert!(first_html.contains("data:application/javascript;base64,"));
    assert!(second_html.contains("data:application/javascript;base64,"));
}

#[test]
fn test_html_widget_dedupe_can_be_enabled() {
    // Power users on frontends with a shared output DOM (classic Jupyter,
    // JupyterLab) can opt into IRkernel-style dedup to keep notebooks small.
    let frontend = DummyArkFrontendNotebook::lock();

    let mut first = String::from(".ps.html_widget_reset_deps()\n");
    first.push_str("options(ark.html_widget.deduplicate = TRUE)\n");
    first.push_str(&setup_fake_widget("marker-first"));
    first.push_str(".ps.view_html_widget(widget)\n");

    let mut second = String::new();
    second.push_str(&setup_fake_widget("marker-second"));
    second.push_str(".ps.view_html_widget(widget)\n");
    // Reset the option so this test doesn't leak state into siblings.
    second.push_str("options(ark.html_widget.deduplicate = NULL)\n");

    let first_html = execute_and_get_html(&frontend, &first);
    let second_html = execute_and_get_html(&frontend, &second);

    // First cell inlines the dep.
    assert!(first_html.contains("data:application/javascript;base64,"));
    // Second cell sees `fakedep@1.0` as already inlined and skips it.
    assert!(!second_html.contains("data:application/javascript;base64,"));
    // Widget body still rendered both times.
    assert!(second_html.contains("widget body"));
}
