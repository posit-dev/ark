//
// parse_boundaries.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use harp::ParseResult;
use harp::RObject;

use crate::lsp::offset::ArkPoint;
use crate::lsp::offset::ArkRange;

#[derive(Debug)]
pub struct ParseBoundaries {
    pub complete: Vec<ArkRange>,
    pub incomplete: Option<ArkRange>,
    pub error: Option<ArkRange>,
}

pub fn parse_boundaries(text: &str) -> anyhow::Result<ParseBoundaries> {
    let mut newlines: Vec<usize> = text
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '\n')
        .map(|(i, _)| i)
        .collect();

    // Include last line
    if let Some(last) = newlines.last() {
        if *last < text.len() - 1 {
            newlines.push(text.len() - 1)
        }
    }

    let n_lines = newlines.len();

    let mut complete: Vec<ArkRange> = vec![];
    let mut incomplete: Option<ArkRange> = None;
    let mut error: Option<ArkRange> = None;

    let mut incomplete_end: Option<ArkPoint> = None;
    let mut error_end: Option<ArkPoint> = None;

    let get_line_width = |line| text.lines().nth(line).unwrap().len();
    let get_line_point = |line| ArkPoint {
        row: line,
        column: get_line_width(line),
    };

    for (i, current_end) in newlines.iter().rev().enumerate() {
        let current_row = n_lines - i;
        let current_point = || ArkPoint {
            row: n_lines - i - 1,
            column: get_line_width(n_lines - i - 1),
        };

        let mut record_error = || {
            if matches!(error, Some(_)) {
                return;
            }
            let Some(end) = error_end else {
                return;
            };
            error = Some(ArkRange {
                start: current_point(),
                end,
            });
        };

        let mut record_incomplete = || {
            let Some(end) = incomplete_end else {
                return;
            };
            incomplete = Some(ArkRange {
                start: current_point(),
                end,
            });
        };

        let mut record_complete = |exprs: RObject| -> anyhow::Result<()> {
            let srcrefs = exprs.srcrefs()?;
            let mut ranges: Vec<ArkRange> =
                srcrefs.into_iter().map(|srcref| srcref.into()).collect();
            complete.append(&mut ranges);
            Ok(())
        };

        // Grab all code up to current line. Include `\n` as there might be a trailing `\r`.
        let subset = &text[..current_end + 1];

        // Parse within source file to get source references
        let srcfile = harp::srcref::SrcFile::new_virtual(subset)?;

        match harp::parse_status(&harp::ParseInput::SrcFile(&srcfile))? {
            ParseResult::Complete(exprs) => {
                record_error();
                record_incomplete();
                record_complete(exprs)?;
                break;
            },

            ParseResult::Incomplete => {
                record_error();

                // Declare incomplete
                incomplete_end = incomplete_end.or_else(|| Some(get_line_point(current_row)));
            },

            ParseResult::SyntaxError { .. } => {
                // Declare error
                error_end = error_end.or_else(|| Some(get_line_point(n_lines - 1)));
            },
        };
    }

    Ok(ParseBoundaries {
        complete,
        incomplete,
        error,
    })
}

#[cfg(test)]
mod tests {
    use harp::assert_match;

    use crate::analysis::parse_boundaries::*;
    use crate::test::r_test;

    #[test]
    fn test_parse_boundaries_complete() {
        r_test(|| {
            let boundaries = parse_boundaries("").unwrap();
            let expected_complete: Vec<ArkRange> = vec![];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("\n\n  ").unwrap();
            let expected_complete: Vec<ArkRange> = vec![];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("\n  foo\n  \n\n").unwrap();
            let expected_complete = vec![ArkRange {
                start: ArkPoint { row: 1, column: 2 },
                end: ArkPoint { row: 2, column: 5 },
            }];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo\nbarbaz").unwrap();
            let expected_complete = vec![
                ArkRange {
                    start: ArkPoint { row: 0, column: 0 },
                    end: ArkPoint { row: 1, column: 3 },
                },
                ArkRange {
                    start: ArkPoint { row: 1, column: 0 },
                    end: ArkPoint { row: 2, column: 6 },
                },
            ];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);
        })
    }
}
