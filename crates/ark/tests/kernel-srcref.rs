use ark::fixtures::DummyArkFrontend;

#[test]
fn test_execute_request_source_references() {
    let frontend = DummyArkFrontend::lock();

    // Test that our parser attaches source references when global option is set
    frontend.execute_request_invisibly("options(keep.source = TRUE)");
    frontend.execute_request_invisibly("f <- function() {}");

    frontend.execute_request(
        "srcref <- attr(f, 'srcref'); inherits(srcref, 'srcref')",
        |result| {
            assert_eq!(result, "[1] TRUE");
        },
    );

    frontend.execute_request(
        "srcfile <- attr(srcref, 'srcfile'); inherits(srcfile, 'srcfile')",
        |result| {
            assert_eq!(result, "[1] TRUE");
        },
    );

    // When global option is unset, we don't attach source references
    frontend.execute_request_invisibly("options(keep.source = FALSE)");
    frontend.execute_request_invisibly("g <- function() {}");

    frontend.execute_request(
        "srcref <- attr(g, 'srcref'); identical(srcref, NULL)",
        |result| {
            assert_eq!(result, "[1] TRUE");
        },
    );
}
