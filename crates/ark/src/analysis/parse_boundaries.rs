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

use crate::coordinates::LineRange;

/// Boundaries are ranges over lines of text.
#[derive(Debug)]
pub struct ParseBoundaries {
    pub complete: Vec<LineRange>,
    pub incomplete: Option<LineRange>,
    pub error: Option<LineRange>,
}

/// Parse boundaries of R inputs
///
/// Takes a string of R code and detects which lines parse as complete,
/// incomplete, and error inputs.
///
/// Invariants:
/// - There is always at least one range as the empty string is a complete input.
/// - The ranges are sorted and non-overlapping.
/// - Inputs are classified in complete, incomplete, and error sections. The
///   sections are sorted in this order (there cannot be complete inputs after
///   an incomplete or error one, there cannot be an incomplete input after
///   an error one, and error inputs are always trailing).
/// - There is only one incomplete and one error input in a set of inputs.
pub fn parse_boundaries(text: &str) -> anyhow::Result<ParseBoundaries> {
    let mut lines: Vec<&str> = text.lines().collect();

    // Rectify for `lines()` ignoring trailing empty lines
    match text.chars().last() {
        Some(last) if last == '\n' => lines.push(""),
        None => lines.push(""),
        _ => {},
    }

    // Create a duplicate vector of lines on the R side too so we don't have to
    // reallocate memory each time we parse a new subset of lines
    let lines_r = CharacterVector::create(lines.iter());
    let n_lines: u32 = lines.len().try_into()?;

    let mut complete: Vec<LineRange> = vec![];
    let mut incomplete: Option<LineRange> = None;
    let mut error: Option<LineRange> = None;

    let mut incomplete_end: Option<u32> = None;
    let mut error_end: Option<u32> = None;

    let mut record_error = |start, error_end: &Option<u32>| {
        if matches!(error, Some(_)) {
            return;
        }
        let Some(end) = error_end else {
            return;
        };
        error = Some(LineRange::new(start, *end));
    };

    let mut record_incomplete = |start, incomplete_end: &Option<u32>| {
        if matches!(incomplete, Some(_)) {
            return;
        }
        let Some(end) = incomplete_end else {
            return;
        };
        incomplete = Some(LineRange::new(start, *end));
    };

    for current_line in (0..n_lines).rev() {
        let mut record_complete = |exprs: RObject| -> anyhow::Result<()> {
            let srcrefs = exprs.srcrefs()?;
            let mut ranges: Vec<LineRange> =
                srcrefs.into_iter().map(|srcref| srcref.into()).collect();

            complete.append(&mut ranges);
            Ok(())
        };

        // Grab all code up to current line. We don't slice the vector in the
        // first iteration as it's not needed.
        let subset = if current_line == n_lines - 1 {
            lines_r.clone()
        } else {
            CharacterVector::try_from(&lines_r.slice()[..=current_line as usize])?
        };

        // Parse within source file to get source references
        let srcfile = harp::srcref::SrcFile::try_from(&subset)?;

        match harp::parse_status(&harp::ParseInput::SrcFile(&srcfile))? {
            ParseResult::Complete(exprs) => {
                record_complete(exprs)?;
                record_incomplete(current_line + 1, &incomplete_end);
                record_error(current_line + 1, &error_end);
                break;
            },

            ParseResult::Incomplete => {
                record_error(current_line + 1, &error_end);

                // Declare incomplete
                if let None = incomplete_end {
                    incomplete_end = Some(current_line + 1);
                }
            },

            ParseResult::SyntaxError { .. } => {
                // Declare error
                if let None = error_end {
                    error_end = Some(current_line + 1);
                }
            },
        };
    }

    // Note that these are necessarily no-ops if we exited the loop after
    // detected complete expressions since we already recorded the boundaries in
    // that case.
    record_incomplete(0, &incomplete_end);
    record_error(0, &error_end);

    // Merge expressions separated by semicolons
    let complete = merge_overlapping(complete);

    let boundaries = ParseBoundaries {
        complete,
        incomplete,
        error,
    };

    // Fill any gaps with one-liner complete expressions
    let boundaries = fill_gaps(boundaries, n_lines);

    Ok(boundaries)
}

fn merge_overlapping(ranges: Vec<LineRange>) -> Vec<LineRange> {
    let merge = |mut merged: Vec<LineRange>, range: LineRange| {
        let Some(last) = merged.pop() else {
            // First element, just add it
            merged.push(range);
            return merged;
        };

        // if range.start <= last.end {
        if last.contains(range.start()) {
            // Overlap, merge with last range
            merged.push(last.cover(range))
        } else {
            // No overlap, push a new range
            merged.push(last);
            merged.push(range);
        }

        merged
    };

    ranges.into_iter().fold(vec![], merge)
}

// Fill any gaps with one-liner complete expressions. This applies
// to empty lines, with or without spaces and tabs, and possibly
// with a trailing comment.
fn fill_gaps(mut boundaries: ParseBoundaries, n_lines: u32) -> ParseBoundaries {
    let mut filled = vec![];
    let ranges = boundaries.complete;

    let mut last_line: u32 = 0;

    let range_from = |start| LineRange::new(start, start + 1);

    // Fill leading whitespace with empty input ranges
    if let Some(first) = ranges
        .get(0)
        .or(boundaries.incomplete.as_ref())
        .or(boundaries.error.as_ref())
    {
        for start in 0..first.start() {
            let range = range_from(start);
            last_line = range.end();
            filled.push(range)
        }
    }

    // Fill gaps between complete expressions
    for range in ranges.into_iter() {
        // We found a gap, fill ranges for lines in that gap
        if !range.contains(last_line) {
            for start in last_line..range.start() {
                filled.push(range_from(start))
            }
        }

        last_line = range.end();
        filled.push(range);
    }

    // Fill trailing whitespace between complete expressions and the rest
    // (incomplete, error, or eof)
    let last_boundary = filled.last().map(|r| r.end()).unwrap_or(0);
    let next_boundary = boundaries
        .incomplete
        .as_ref()
        .or(boundaries.error.as_ref())
        .map(|r| r.start())
        .unwrap_or(n_lines);

    for start in last_boundary..next_boundary {
        filled.push(range_from(start))
    }

    boundaries.complete = filled;
    boundaries
}

#[cfg(test)]
mod tests {
    use harp::assert_match;

    use crate::analysis::parse_boundaries::*;
    use crate::test::r_test;

    #[test]
    fn test_parse_boundaries_complete() {
        r_test(|| {
            let boundaries = parse_boundaries("foo").unwrap();
            #[rustfmt::skip]
            assert_eq!(boundaries.complete, vec![
                LineRange::new(0, 1),
            ]);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo\nbarbaz  ").unwrap();
            assert_eq!(boundaries.complete, vec![
                LineRange::new(0, 1),
                LineRange::new(1, 2)
            ]);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);
        })
    }

    #[test]
    fn test_parse_boundaries_whitespace() {
        r_test(|| {
            let boundaries = parse_boundaries("").unwrap();
            #[rustfmt::skip]
            assert_eq!(boundaries.complete, vec![
                LineRange::new(0, 1),
            ]);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("\n\n  \n").unwrap();
            assert_eq!(boundaries.complete, vec![
                LineRange::new(0, 1),
                LineRange::new(1, 2),
                LineRange::new(2, 3),
                LineRange::new(3, 4),
            ]);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("\n  foo\n  \n\n").unwrap();
            assert_eq!(boundaries.complete, vec![
                LineRange::new(0, 1),
                LineRange::new(1, 2),
                LineRange::new(2, 3),
                LineRange::new(3, 4),
                LineRange::new(4, 5),
            ]);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);
        })
    }

    #[test]
    fn test_parse_boundaries_complete_semicolon() {
        r_test(|| {
            // These should only produce a single complete input range

            let boundaries = parse_boundaries("foo;bar").unwrap();
            let expected_complete = vec![LineRange::new(0, 1)];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo;bar(\n)").unwrap();
            let expected_complete = vec![LineRange::new(0, 2)];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo(\n);bar").unwrap();
            let expected_complete = vec![LineRange::new(0, 2)];
            assert_eq!(boundaries.complete, expected_complete);
            assert_match!(boundaries.incomplete, None);
            assert_match!(boundaries.error, None);
        });
    }

    #[test]
    fn test_parse_boundaries_incomplete() {
        r_test(|| {
            let boundaries = parse_boundaries("foo +").unwrap();
            assert_eq!(boundaries.complete.len(), 0);
            assert_eq!(boundaries.incomplete, Some(LineRange::new(0, 1)));
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("\n\n  foo + \n  \n  ").unwrap();
            assert_eq!(boundaries.complete, vec![
                LineRange::new(0, 1),
                LineRange::new(1, 2),
            ]);
            assert_eq!(boundaries.incomplete, Some(LineRange::new(2, 5)));
            assert_match!(boundaries.error, None);

            let boundaries = parse_boundaries("foo\nbar; foo +").unwrap();
            assert_eq!(boundaries.complete, vec![LineRange::new(0, 1)]);
            assert_eq!(boundaries.incomplete, Some(LineRange::new(1, 2)));
            assert_match!(boundaries.error, None);
        });
    }

    #[test]
    fn test_parse_boundaries_error() {
        r_test(|| {
            let boundaries = parse_boundaries("foo )").unwrap();
            assert_eq!(boundaries.complete.len(), 0);
            assert_match!(boundaries.incomplete, None);
            assert_eq!(boundaries.error, Some(LineRange::new(0, 1)));

            let boundaries = parse_boundaries("foo\nbar )\n  ").unwrap();
            assert_eq!(boundaries.complete, vec![LineRange::new(0, 1)]);
            assert_match!(boundaries.incomplete, None);
            assert_eq!(boundaries.error, Some(LineRange::new(1, 3)));

            let boundaries = parse_boundaries("foo\nbar +\nbaz )").unwrap();
            assert_eq!(boundaries.complete, vec![LineRange::new(0, 1)]);
            assert_eq!(boundaries.incomplete, Some(LineRange::new(1, 2)));
            assert_eq!(boundaries.error, Some(LineRange::new(2, 3)));

            let boundaries = parse_boundaries("foo +\n  bar +;").unwrap();
            assert_eq!(boundaries.complete.len(), 0);
            assert_eq!(boundaries.incomplete, Some(LineRange::new(0, 1)));
            assert_eq!(boundaries.error, Some(LineRange::new(1, 2)));
        });
    }
}
