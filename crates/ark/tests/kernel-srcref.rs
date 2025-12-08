use amalthea::wire::execute_request::JupyterPositronLocation;
use amalthea::wire::execute_request::JupyterPositronPosition;
use amalthea::wire::execute_request::JupyterPositronRange;
use ark::fixtures::DummyArkFrontend;

#[test]
fn test_execute_request_srcref() {
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

#[test]
fn test_execute_request_srcref_location_line_shift() {
    let frontend = DummyArkFrontend::lock();

    // Starting at line 3, column 0
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 0,
            },
            end: JupyterPositronPosition {
                line: 2,
                character: 25,
            },
        },
    };
    frontend.execute_request_with_location("fn <- function() {}; NULL", |_| (), code_location);

    // `function` starts at column 7, the body ends at 19 (right-boundary position)
    // Lines are 1-based so incremented by 1.
    frontend.execute_request(".ps.internal(get_srcref_range(fn))", |result| {
        assert_eq!(
            result,
            "$start
     line character\u{20}
        3         7\u{20}

$end
     line character\u{20}
        3        19\u{20}
"
        );
    });
}

#[test]
fn test_execute_request_srcref_location_line_and_column_shift() {
    let frontend = DummyArkFrontend::lock();

    // Starting at line 3, column 3
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 3,
            },
            end: JupyterPositronPosition {
                line: 2,
                character: 25,
            },
        },
    };
    frontend.execute_request_with_location("fn <- function() {}; NULL", |_| (), code_location);

    // `function` starts at column 7, the body ends at 19 (right-boundary position)
    // Lines are 1-based so incremented by 1.
    frontend.execute_request(".ps.internal(get_srcref_range(fn))", |result| {
        assert_eq!(
            result,
            "$start
     line character\u{20}
        3        10\u{20}

$end
     line character\u{20}
        3        22\u{20}
"
        );
    });
}
