use std::io::Write;

use ark::fixtures::DummyArkFrontend;

#[test]
fn test_source_local() {
    let frontend = DummyArkFrontend::lock();

    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "foobar\n").unwrap();

    let path = file.path().to_str().unwrap();

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
fn test_source_local_fallback() {
    let frontend = DummyArkFrontend::lock();

    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "foobar\n").unwrap();

    let path = file.path().to_str().unwrap();

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
