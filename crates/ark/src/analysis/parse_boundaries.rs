//
// parse_boundaries.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use harp::vector::CharacterVector;
use harp::vector::Vector;
use harp::ParseResult;
use harp::RObject;

#[derive(Debug)]
pub struct ParseBoundaries {
    pub complete: Vec<std::ops::Range<usize>>,
    pub incomplete: Option<std::ops::Range<usize>>,
    pub error: Option<std::ops::Range<usize>>,
}

pub fn parse_boundaries(text: &str) -> anyhow::Result<ParseBoundaries> {
    let lines: Vec<&str> = text.lines().collect();

    // Create a duplicate vector of lines on the R side too so we don't have to
    // reallocate memory each time we parse a new subset of lines
    let lines_r = CharacterVector::create(lines.iter());
    let n_lines = lines.len();

    let mut complete: Vec<std::ops::Range<usize>> = vec![];
    let mut incomplete: Option<std::ops::Range<usize>> = None;
    let mut error: Option<std::ops::Range<usize>> = None;

    let mut incomplete_end: Option<usize> = None;
    let mut error_end: Option<usize> = None;

    for current_line in (0..n_lines).rev() {
        let mut record_error = || {
            if matches!(error, Some(_)) {
                return;
            }
            let Some(end) = error_end else {
                return;
            };
            error = Some(std::ops::Range {
                start: current_line,
                end,
            });
        };

        let mut record_incomplete = || {
            let Some(end) = incomplete_end else {
                return;
            };
            incomplete = Some(std::ops::Range {
                start: current_line,
                end,
            });
        };

        let mut record_complete = |exprs: RObject| -> anyhow::Result<()> {
            let srcrefs = exprs.srcrefs()?;
            let ranges: Vec<std::ops::Range<usize>> =
                srcrefs.into_iter().map(|srcref| srcref.line).collect();

            // Merge expressions separated by semicolons
            let mut ranges = merge_overlapping(ranges);

            complete.append(&mut ranges);
            Ok(())
        };

        // Grab all code up to current line
        let subset = &lines_r.slice()[..current_line + 1];
        let subset = CharacterVector::try_from(subset)?;

        // Parse within source file to get source references
        let srcfile = harp::srcref::SrcFile::try_from(&subset)?;

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
                if let None = incomplete_end {
                    incomplete_end = Some(current_line);
                }
            },

            ParseResult::SyntaxError { .. } => {
                // Declare error
                if let None = error_end {
                    error_end = Some(current_line);
                }
            },
        };
    }

    Ok(ParseBoundaries {
        complete,
        incomplete,
        error,
    })
}

fn merge_overlapping<T>(ranges: Vec<std::ops::Range<T>>) -> Vec<std::ops::Range<T>>
where
    T: PartialOrd + Ord + Copy,
{
    let merge = |mut merged: Vec<std::ops::Range<T>>, range: std::ops::Range<T>| {
        if let Some(last) = merged.last_mut() {
            // if range.start <= last.end {
            if last.contains(&range.start) {
                // Overlap, merge with last range
                last.end = last.end.max(range.end);
            } else {
                // No overlap, push a new range
                merged.push(range);
            }
        } else {
            // First element, just add it
            merged.push(range);
        }

        merged
    };

    ranges.into_iter().fold(vec![], merge)
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
            let expected_complete: Vec<std::ops::Range<usize>> = vec![];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("\n\n  ").unwrap();
            let expected_complete: Vec<std::ops::Range<usize>> = vec![];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("\n  foo\n  \n\n").unwrap();
            let expected_complete = vec![std::ops::Range { start: 1, end: 2 }];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo\nbarbaz  \n").unwrap();
            let expected_complete = vec![std::ops::Range { start: 0, end: 1 }, std::ops::Range {
                start: 1,
                end: 2,
            }];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);
        })
    }

    #[test]
    fn test_parse_boundaries_complete_semicolon() {
        r_test(|| {
            // These should only produce a single complete input range

            let boundaries = parse_boundaries("foo;bar").unwrap();
            let expected_complete = vec![std::ops::Range { start: 0, end: 1 }];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo;bar(\n)").unwrap();
            let expected_complete = vec![std::ops::Range { start: 0, end: 2 }];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo(\n);bar").unwrap();
            let expected_complete = vec![std::ops::Range { start: 0, end: 2 }];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);
        });
    }
}
