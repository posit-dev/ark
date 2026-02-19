use std::io::Write;

use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

#[test]
fn test_source_local() {
    let frontend = DummyArkFrontend::lock();

    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "foobar\n").unwrap();

    let path = file.path().to_str().unwrap().replace("\\", "/");

    // Breakpoint injection path
    let code = format!(
        r#"local({{
    foobar <- "worked"
    source("{path}", local = TRUE)$value
}})"#
    );

    frontend.execute_request(&code, |result| {
        assert_eq!(result, "[1] \"worked\"");
    });

    // Fallback path (because we supply `encoding`)
    let code = format!(
        r#"local({{
    foobar <- "worked"
    source("{path}", local = TRUE, encoding = "UTF-8")$value
}})"#
    );

    frontend.execute_request(&code, |result| {
        assert_eq!(result, "[1] \"worked\"");
    });
}

#[test]
fn test_source_global() {
    let frontend = DummyArkFrontend::lock();

    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "foo\n").unwrap();

    let path = file.path().to_str().unwrap().replace("\\", "/");

    // Breakpoint injection path
    frontend.execute_request_invisibly(r#"foo <- "worked!""#);

    let code = format!(
        r#"local({{
    foo <- "did not work!"
    source("{path}")$value
}})"#
    );

    frontend.execute_request(&code, |result| {
        assert_eq!(result, "[1] \"worked!\"");
    });

    // Fallback path (because we supply `encoding`)
    let code = format!(
        r#"local({{
    foo <- "did not work!"
    source("{path}", encoding = "UTF-8")$value
}})"#
    );

    frontend.execute_request(&code, |result| {
        assert_eq!(result, "[1] \"worked!\"");
    });
}

#[test]
fn test_source_returns_last_value_invisibly() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create a file where the last expression returns a value.
    let file = SourceFile::new(
        "
foo <- function() {
    1
}
x <- 1
x + 41
",
    );

    // Set a breakpoint to trigger the annotation code path.
    // Without a breakpoint, the hook falls back to base::source().
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);

    // Source the file. The hooked source() returns invisibly.
    frontend.source_file(&file);
    dap.recv_breakpoint_verified();

    // Verify the return value structure matches base::source():
    // list(value = <last_expr>, visible = <TRUE/FALSE>)
    // Use parentheses to force visibility of the result.
    let code = format!(r#"(source("{}"))"#, file.path);
    frontend.execute_request(&code, |result| {
        assert!(result.contains("$value"), "Expected $value in result");
        assert!(result.contains("[1] 42"), "Expected value to be 42");
        assert!(result.contains("$visible"), "Expected $visible in result");
        assert!(result.contains("[1] TRUE"), "Expected visible to be TRUE");
    });

    // Verify it's invisible: bare source() produces no execute_result.
    let code = format!(r#"source("{}")"#, file.path);
    frontend.execute_request_invisibly(&code);
}

#[test]
fn test_source_returns_invisible_value_with_visible_false() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create a file where the last expression is invisible.
    // The withVisible() value should be FALSE.
    let file = SourceFile::new(
        "
foo <- function() {
    1
}
invisible(42)
",
    );

    // Set a breakpoint to trigger the annotation code path.
    // Without a breakpoint, the hook falls back to base::source().
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);

    // Source the file. The hooked source() returns invisibly.
    frontend.source_file(&file);
    dap.recv_breakpoint_verified();

    // Verify the return value structure matches base::source():
    // list(value = <last_expr>, visible = <TRUE/FALSE>)
    // When the last expression is invisible(42), visible should be FALSE.
    let code = format!(r#"(source("{}"))"#, file.path);
    frontend.execute_request(&code, |result| {
        assert!(result.contains("$value"), "Expected $value in result");
        assert!(result.contains("[1] 42"), "Expected value to be 42");
        assert!(result.contains("$visible"), "Expected $visible in result");
        assert!(
            result.contains("[1] FALSE"),
            "Expected visible to be FALSE for invisible() result, got: {}",
            result
        );
    });
}

#[test]
fn test_ark_annotate_source_returns_null_without_breakpoints() {
    let frontend = DummyArkFrontend::lock();
    let _dap = frontend.start_dap();

    // Without any breakpoints set, .ark_annotate_source should return NULL
    let code = r#"is.null(base::.ark_annotate_source("x <- 1", "file:///test.R"))"#;
    frontend.execute_request(code, |result| {
        assert_eq!(result, "[1] TRUE");
    });
}

#[test]
fn test_ark_annotate_source_returns_annotated_code_with_breakpoints() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create a file and set a breakpoint
    let file = SourceFile::new(
        "foo <- function() {
    1
}
",
    );

    // Breakpoint on line 2 (the `1` inside the function body)
    let breakpoints = dap.set_breakpoints(&file.path, &[2]);
    assert_eq!(breakpoints.len(), 1);

    // .ark_annotate_source should return non-NULL annotated code
    // Note: URI must match what was used for set_breakpoints (use file.uri)
    let code = format!(
        r#"!is.null(base::.ark_annotate_source("foo <- function() {{\n    1\n}}", "{}"))"#,
        file.uri
    );
    frontend.execute_request(&code, |result| {
        assert_eq!(result, "[1] TRUE");
    });
}

#[test]
fn test_ark_annotate_source_preserves_last_value() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create a file with a function definition (breakpoint inside won't be hit)
    // and a final expression that returns a value
    let file = SourceFile::new(
        "foo <- function() {
    1
}
x <- 1
x + 41
",
    );

    // Set a breakpoint inside the function body (line 2) - won't be hit since
    // we don't call foo(), but triggers annotation path
    let breakpoints = dap.set_breakpoints(&file.path, &[2]);
    assert_eq!(breakpoints.len(), 1);

    // Get annotated code and evaluate it - should return 42
    // Note: URI must match what was used for set_breakpoints (use file.uri)
    let source = r#"foo <- function() {
    1
}
x <- 1
x + 41
"#;
    let code = format!(
        r#"eval(parse(text = base::.ark_annotate_source("{}", "{}")))"#,
        source.replace('\n', "\\n"),
        file.uri
    );
    frontend.execute_request(&code, |result| {
        assert_eq!(result, "[1] 42");
    });

    // Receive breakpoint verified event
    dap.recv_breakpoint_verified();
}
