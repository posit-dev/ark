use amalthea::wire::execute_request::JupyterPositronLocation;
use amalthea::wire::execute_request::JupyterPositronPosition;
use amalthea::wire::execute_request::JupyterPositronRange;
use ark_test::DummyArkFrontend;

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
fn test_execute_request_srcref_location_multiline_trailing_newline() {
    // Checks code location handling with trailing newline. We used to have a
    // bug due to the `lines()` method ignoring trailing newlines.

    let frontend = DummyArkFrontend::lock();

    // Code spans lines 3-6 in the document, starting at column 4.
    // The code ends with a trailing newline, so the range ends at line 6, character 0.
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 4,
            },
            end: JupyterPositronPosition {
                line: 5,
                character: 0,
            },
        },
    };

    // Multiline function definition with trailing newline
    let code = "fn <- function() {
    1
}; NULL
";
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

    // Starting at line 3, column 3 (these input positions are in UTF-8 bytes).
    // The code is 29 UTF-8 bytes (the emoji 🙂 is 4 bytes), so end.character = 3 + 29 = 32.
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 2,
                character: 3,
            },
            end: JupyterPositronPosition {
                line: 2,
                character: 29 + 3,
            },
        },
    };

    // The function body contains a single emoji character which is 4 UTF-8 bytes.
    frontend.execute_request_with_location("fn <- function() \"🙂\"; NULL", |_| (), code_location);

    // `function` starts at byte 7 in the code, so with start.character=3 we get 10.
    // The function body `"🙂"` ends at byte 23 locally (the closing quote after the 4-byte emoji),
    // so with start.character=3 the reported end becomes 23.
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

/// Verify that the srcfile created by our `#line` annotation has properly
/// split lines so that `getSrcLines()` returns individual lines.
/// Packages like reprex use `getSrcLines()` to retrieve source code from
/// srcrefs, and they break when lines aren't split into separate elements.
/// https://github.com/posit-dev/positron/issues/11578
#[test]
fn test_execute_request_srcref_getsrclines() {
    let frontend = DummyArkFrontend::lock();

    // Execute multiline code with an empty line, from the "editor" (with
    // location info). This triggers `#line` annotation and creates a srcfile.
    let code = "f <- function() {\n  1\n\n  2\n}; NULL";
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/file.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 0,
                character: 0,
            },
            end: JupyterPositronPosition {
                line: 4,
                character: 7,
            },
        },
    };
    frontend.execute_request_with_location(code, |_| (), code_location);

    // The annotated code prepends a `#line` directive, producing 6 lines:
    //   1: #line 1 "file:///path/to/file.R"
    //   2: f <- function() {
    //   3:   1
    //   4:           (empty)
    //   5:   2
    //   6: }; NULL
    // `getSrcLines()` must return one element per line.
    frontend.execute_request(
        "srcref <- attr(f, 'srcref')
srcfile <- attr(srcref, 'srcfile')
lines <- getSrcLines(srcfile, 1, 6)
length(lines)",
        |result| {
            assert_eq!(result, "[1] 6");
        },
    );

    // The empty line between `1` and `2` must be preserved
    frontend.execute_request("identical(lines[4], '')", |result| {
        assert_eq!(result, "[1] TRUE");
    });
}

/// Inline the core logic of reprex's `stringify_expression()` to verify that
/// source lines retrieved via srcrefs from `#line`-annotated code preserve
/// empty lines and match what you'd get without annotation.
///
/// Reprex builds a merged srcref spanning first-to-last child of a `{ }`
/// block, then calls `as.character(srcref, useSource = TRUE)` which uses
/// `getSrcLines()` with parse line numbers (entries 7 and 8 of the srcref).
/// https://github.com/posit-dev/positron/issues/11578
#[test]
fn test_execute_request_srcref_reprex_stringify() {
    let frontend = DummyArkFrontend::lock();

    // Inline reimplementation of reprex's `stringify_expression()`.
    // Captures its argument's expression via `substitute()` (as reprex does),
    // builds a merged srcref spanning first-to-last child, retrieves source
    // lines, and strips the leading `{` line.
    frontend.execute_request_invisibly(
        "stringify_expr <- function(x) {
  expr <- substitute(x)
  src_list <- utils::getSrcref(expr)
  if (is.null(src_list)) return(deparse(expr))
  first_src <- src_list[[1]]
  last_src  <- src_list[[length(src_list)]]
  srcfile   <- attr(first_src, 'srcfile')
  src <- srcref(srcfile, c(
    first_src[[1]], first_src[[2]], last_src[[3]], last_src[[4]],
    first_src[[5]], last_src[[6]], first_src[[7]], last_src[[8]]
  ))
  lines <- as.character(src, useSource = TRUE)
  lines[[1L]] <- sub('^[{]', '', lines[[1L]])
  if (!nzchar(lines[[1L]])) lines <- lines[-1L]
  lines
}",
    );

    // Call from the "editor" with location info to trigger `#line` annotation.
    // The `{ }` block has an empty line between two expressions.
    let code = "result <- stringify_expr({\n  1 + 1\n\n  1 + 1\n}); NULL";
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/test.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 0,
                character: 0,
            },
            end: JupyterPositronPosition {
                line: 4,
                character: 7,
            },
        },
    };
    frontend.execute_request_with_location(code, |_| (), code_location);

    // Also run without location (no #line annotation) for comparison
    frontend.execute_request_invisibly("result_no_loc <- stringify_expr({\n  1 + 1\n\n  1 + 1\n})");

    // The output with location should match the output without location:
    // 3 lines: "  1 + 1", "", "  1 + 1"
    frontend.execute_request("identical(result, result_no_loc)", |result| {
        assert_eq!(result, "[1] TRUE");
    });

    // Must be exactly 3 lines with the empty line preserved
    frontend.execute_request("length(result)", |result| {
        assert_eq!(result, "[1] 3");
    });
    frontend.execute_request("identical(result[2], '')", |result| {
        assert_eq!(result, "[1] TRUE");
    });
}

/// Same as above but with a non-zero line offset (code at line 10 in the
/// document). This was the scenario that originally triggered the reprex
/// bug: `getSrcLocation(srcref, which = "line")` returned the virtual line
/// from the `#line` directive, causing `getSrcLines()` to read past the
/// end of the srcfile.
/// https://github.com/posit-dev/positron/issues/11578
#[test]
fn test_execute_request_srcref_reprex_stringify_with_offset() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly(
        "stringify_expr <- function(x) {
  expr <- substitute(x)
  src_list <- utils::getSrcref(expr)
  if (is.null(src_list)) return(deparse(expr))
  first_src <- src_list[[1]]
  last_src  <- src_list[[length(src_list)]]
  srcfile   <- attr(first_src, 'srcfile')
  src <- srcref(srcfile, c(
    first_src[[1]], first_src[[2]], last_src[[3]], last_src[[4]],
    first_src[[5]], last_src[[6]], first_src[[7]], last_src[[8]]
  ))
  lines <- as.character(src, useSource = TRUE)
  lines[[1L]] <- sub('^[{]', '', lines[[1L]])
  if (!nzchar(lines[[1L]])) lines <- lines[-1L]
  lines
}",
    );

    // Code at line 10 (0-based 9) in the document
    let code = "result <- stringify_expr({\n  1 + 1\n\n  1 + 1\n}); NULL";
    let code_location = JupyterPositronLocation {
        uri: "file:///path/to/test.R".to_owned(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 9,
                character: 0,
            },
            end: JupyterPositronPosition {
                line: 13,
                character: 7,
            },
        },
    };
    frontend.execute_request_with_location(code, |_| (), code_location);

    // Must be 3 lines with the empty line preserved, even with a large
    // line offset that causes virtual lines (10+) to diverge from parse
    // lines (1-5).
    frontend.execute_request("length(result)", |result| {
        assert_eq!(result, "[1] 3");
    });
    frontend.execute_request("identical(result[2], '')", |result| {
        assert_eq!(result, "[1] TRUE");
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
