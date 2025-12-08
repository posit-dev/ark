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
                character: 25 + 3,
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

#[test]
fn test_execute_request_srcref_location_multiline() {
    let frontend = DummyArkFrontend::lock();

    // Code spans lines 3-5 in the document, starting at column 4
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 4,
            },
            end: JupyterPositronPosition {
                line: 4,
                character: 7,
            },
        },
    };

    // Multiline function definition
    let code = "fn <- function() {
    1
}; NULL";
    frontend.execute_request_with_location(code, |_| (), code_location);

    // `function` starts at column 7 on line 1, with start.character=4 offset -> line 3, col 11
    // The closing brace is at column 1 on line 3 of code (line 5 in document)
    frontend.execute_request(".ps.internal(get_srcref_range(fn))", |result| {
        assert_eq!(
            result,
            "$start
     line character\u{20}
        3        11\u{20}

$end
     line character\u{20}
        5         1\u{20}
"
        );
    });
}

#[test]
fn test_execute_request_srcref_location_with_emoji_utf8_shift() {
    let frontend = DummyArkFrontend::lock();

    // Starting at line 3, column 3 (these input positions are in Unicode code points)
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 3,
            },
            end: JupyterPositronPosition {
                line: 2,
                character: 26 + 3,
            },
        },
    };

    // The function body contains a single emoji character. The input character positions above
    // are specified as Unicode code points. The srcref we receive reports UTF-8 byte positions,
    // so the presence of the multibyte emoji shifts the end position by the emoji's extra bytes.
    frontend.execute_request_with_location("fn <- function() \"🙂\"; NULL", |_| (), code_location);

    // `function` starts at column 7 (code point counting), so with a start.character of 3 we get 10.
    // The function body `"🙂"` ends at UTF-8 byte 20 locally (the closing quote), so with
    // start.character=3 the reported end becomes 23.
    frontend.execute_request(".ps.internal(get_srcref_range(fn))", |result| {
        assert_eq!(
            result,
            "$start
     line character\u{20}
        3        10\u{20}

$end
     line character\u{20}
        3        23\u{20}
"
        );
    });
}

#[test]
fn test_execute_request_srcref_location_invalid_end_line() {
    let frontend = DummyArkFrontend::lock();

    // Invalid location: end line exceeds the number of lines in the input
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 3,
            },
            end: JupyterPositronPosition {
                line: 10,
                character: 25,
            },
        },
    };
    frontend.execute_request_with_location("fn <- function() {}; NULL", |_| (), code_location);

    // With invalid location, fallback behavior uses start of file (line 1, column 1).
    // `function` starts at column 7, the body ends at 19 (right-boundary position).
    frontend.execute_request(".ps.internal(get_srcref_range(fn))", |result| {
        assert_eq!(
            result,
            "$start
     line character\u{20}
        1         7\u{20}

$end
     line character\u{20}
        1        19\u{20}
"
        );
    });
}

#[test]
fn test_execute_request_srcref_location_invalid_end_character() {
    let frontend = DummyArkFrontend::lock();

    // Invalid location: end character exceeds the number of characters in the last line
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 3,
            },
            end: JupyterPositronPosition {
                line: 2,
                character: 1000,
            },
        },
    };
    frontend.execute_request_with_location("fn <- function() {}; NULL", |_| (), code_location);

    // With invalid location, fallback behavior uses start of file (line 1, column 1).
    // `function` starts at column 7, the body ends at 19 (right-boundary position).
    frontend.execute_request(".ps.internal(get_srcref_range(fn))", |result| {
        assert_eq!(
            result,
            "$start
     line character\u{20}
        1         7\u{20}

$end
     line character\u{20}
        1        19\u{20}
"
        );
    });
}
