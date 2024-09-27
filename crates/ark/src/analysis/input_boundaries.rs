//
// input_boundaries.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use harp::vector::CharacterVector;
use harp::vector::Vector;
use harp::ParseResult;
use harp::RObject;
use serde::Serialize;

use crate::coordinates::LineRange;

/// Boundaries are ranges over lines of text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputBoundary {
    pub range: LineRange,

    #[serde(flatten)]
    pub kind: InputBoundaryKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "data")]
#[serde(rename_all = "snake_case")]
pub enum InputBoundaryKind {
    Whitespace,
    Complete,
    Incomplete,
    Invalid { message: String },
}

impl InputBoundary {
    fn new(range: LineRange, kind: InputBoundaryKind) -> Self {
        Self { range, kind }
    }

    pub fn whitespace(range: LineRange) -> Self {
        Self::new(range, InputBoundaryKind::Whitespace)
    }
    pub fn complete(range: LineRange) -> Self {
        Self::new(range, InputBoundaryKind::Complete)
    }
    pub fn incomplete(range: LineRange) -> Self {
        Self::new(range, InputBoundaryKind::Incomplete)
    }
    pub fn invalid(range: LineRange, message: String) -> Self {
        Self::new(range, InputBoundaryKind::Invalid { message })
    }
}

/// Input boundaries of R code
///
/// Takes a string of R code and detects which lines parse as complete,
/// incomplete, and invalid inputs.
///
/// Invariants:
/// - There is always at least one range as the empty string is a complete input.
/// - The ranges are sorted and non-overlapping.
/// - Inputs are classified in complete, incomplete, and invalid sections. The
///   sections are sorted in this order (there cannot be complete inputs after
///   an incomplete or invalid one, there cannot be an incomplete input after
///   an invalid one, and invalid inputs are always trailing).
/// - There is only one incomplete and one invalid input in a set of inputs.
pub fn input_boundaries(text: &str) -> anyhow::Result<Vec<InputBoundary>> {
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
    let mut invalid: Option<LineRange> = None;
    let mut invalid_message: Option<String> = None;

    let mut incomplete_end: Option<u32> = None;
    let mut invalid_end: Option<u32> = None;

    let mut record_invalid = |start, invalid_end: &Option<u32>| {
        if matches!(invalid, Some(_)) {
            return;
        }
        let Some(end) = invalid_end else {
            return;
        };
        invalid = Some(LineRange::new(start, *end));
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
                record_invalid(current_line + 1, &invalid_end);
                break;
            },

            ParseResult::Incomplete => {
                record_invalid(current_line + 1, &invalid_end);

                // Declare incomplete
                if let None = incomplete_end {
                    incomplete_end = Some(current_line + 1);
                }
            },

            ParseResult::SyntaxError { message, .. } => {
                // Declare invalid
                if let None = invalid_end {
                    invalid_end = Some(current_line + 1);
                    invalid_message = Some(message)
                }
            },
        };
    }

    // Note that these are necessarily no-ops if we exited the loop after
    // detected complete expressions since we already recorded the boundaries in
    // that case.
    record_incomplete(0, &incomplete_end);
    record_invalid(0, &invalid_end);

    // Merge expressions separated by semicolons
    let complete = merge_overlapping(complete);

    // Fill any gaps with one-liner complete expressions. Creates
    // `InputBoundary` elements of the right type (complete or whitespace)
    let mut boundaries = fill_gaps(complete, &incomplete, &invalid, n_lines);

    // Now push incomplete and invalid boundaries, if any
    if let Some(boundary) = incomplete {
        boundaries.push(InputBoundary::incomplete(boundary));
    }
    if let Some(boundary) = invalid {
        // SAFETY: `invalid_message` has to be `Some()` because `invalid` is `Some()`
        boundaries.push(InputBoundary::invalid(boundary, invalid_message.unwrap()));
    }

    Ok(boundaries)
}

fn merge_overlapping(ranges: Vec<LineRange>) -> Vec<LineRange> {
    let merge = |mut merged: Vec<LineRange>, range: LineRange| {
        let Some(last) = merged.pop() else {
            // First element, just add it
            merged.push(range);
            return merged;
        };

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
fn fill_gaps(
    complete: Vec<LineRange>,
    incomplete: &Option<LineRange>,
    invalid: &Option<LineRange>,
    n_lines: u32,
) -> Vec<InputBoundary> {
    let mut filled: Vec<InputBoundary> = vec![];
    let mut last_line: u32 = 0;

    let range_from = |start| LineRange::new(start, start + 1);

    // Fill leading whitespace with empty input ranges
    if let Some(first) = complete.get(0).or(incomplete.as_ref()).or(invalid.as_ref()) {
        for start in 0..first.start() {
            let range = range_from(start);
            last_line = range.end();
            filled.push(InputBoundary::whitespace(range))
        }
    }

    // Fill gaps between complete expressions
    for range in complete.into_iter() {
        // We found a gap, fill ranges for lines in that gap
        if !range.contains(last_line) {
            for start in last_line..range.start() {
                filled.push(InputBoundary::whitespace(range_from(start)))
            }
        }

        last_line = range.end();
        filled.push(InputBoundary::complete(range));
    }

    // Fill trailing whitespace between complete expressions and the rest
    // (incomplete, invalid, or eof)
    let last_complete_boundary = filled.last().map(|b| b.range.end()).unwrap_or(0);
    let next_boundary = incomplete
        .as_ref()
        .or(invalid.as_ref())
        .map(|r| r.start())
        .unwrap_or(n_lines);

    for start in last_complete_boundary..next_boundary {
        filled.push(InputBoundary::whitespace(range_from(start)))
    }

    filled
}

#[cfg(test)]
mod tests {
    use crate::analysis::input_boundaries::*;
    use crate::fixtures::r_test;

    fn p(text: &str) -> Vec<InputBoundary> {
        let mut boundaries = input_boundaries(text).unwrap();

        // Replace error messages with placeholder so they don't interfere with
        // equality tests
        if let Some(last) = boundaries.last() {
            if let InputBoundaryKind::Invalid { .. } = &last.kind {
                let range = last.range.to_owned();
                boundaries.pop();
                boundaries.push(InputBoundary::invalid(range, String::from("placeholder")))
            }
        }

        boundaries
    }

    fn boundary(start: u32, end: u32, kind: InputBoundaryKind) -> InputBoundary {
        let range = LineRange::new(start, end);
        InputBoundary::new(range, kind)
    }
    fn whitespace(start: u32, end: u32) -> InputBoundary {
        boundary(start, end, InputBoundaryKind::Whitespace)
    }
    fn complete(start: u32, end: u32) -> InputBoundary {
        boundary(start, end, InputBoundaryKind::Complete)
    }
    fn incomplete(start: u32, end: u32) -> InputBoundary {
        boundary(start, end, InputBoundaryKind::Incomplete)
    }
    fn invalid(start: u32, end: u32) -> InputBoundary {
        boundary(start, end, InputBoundaryKind::Invalid {
            message: String::from("placeholder"),
        })
    }

    #[test]
    fn test_input_boundaries_complete() {
        r_test(|| {
            assert_eq!(p("foo"), vec![complete(0, 1),]);
            assert_eq!(p("foo\nbarbaz  "), vec![complete(0, 1), complete(1, 2)]);
        })
    }

    #[test]
    fn test_input_boundaries_whitespace() {
        r_test(|| {
            #[rustfmt::skip]
            assert_eq!(p(""), vec![
                whitespace(0, 1),
            ]);

            assert_eq!(p("\n\n  \n"), vec![
                whitespace(0, 1),
                whitespace(1, 2),
                whitespace(2, 3),
                whitespace(3, 4),
            ]);

            assert_eq!(p("\n  foo\n  \n\n"), vec![
                whitespace(0, 1),
                complete(1, 2),
                whitespace(2, 3),
                whitespace(3, 4),
                whitespace(4, 5),
            ]);
        })
    }

    #[test]
    fn test_input_boundaries_complete_semicolon() {
        r_test(|| {
            // These should only produce a single complete input range
            assert_eq!(p("foo;bar"), vec![complete(0, 1)]);
            assert_eq!(p("foo;bar(\n)"), vec![complete(0, 2)]);
            assert_eq!(p("foo(\n);bar"), vec![complete(0, 2)]);
        });
    }

    #[test]
    fn test_input_boundaries_incomplete() {
        #[rustfmt::skip]
        r_test(|| {
            assert_eq!(p("foo +"), vec![
                incomplete(0, 1),
            ]);

            assert_eq!(p("\n\n  foo + \n  \n  "), vec![
                whitespace(0, 1),
                whitespace(1, 2),
                incomplete(2, 5),
            ]);

            assert_eq!(p("foo\nbar; foo +"), vec![
                complete(0, 1),
                incomplete(1, 2),
            ]);

            assert_eq!(p("#\n# foo\n  # bar \nbaz +  \n # qux"), vec![
                whitespace(0, 1),
                whitespace(1, 2),
                whitespace(2, 3),
                incomplete(3, 5),
            ]);
        });
    }

    #[test]
    fn test_input_boundaries_invalid() {
        #[rustfmt::skip]
        r_test(|| {
            assert_eq!(p("foo )"), vec![
                invalid(0, 1),
            ]);

            assert_eq!(p("foo\nbar )\n  "), vec![
                complete(0, 1),
                invalid(1, 3),
            ]);

            assert_eq!(p("foo\nbar +\nbaz )"), vec![
                complete(0, 1),
                incomplete(1, 2),
                invalid(2, 3),
            ]);

            assert_eq!(p("foo +\n  bar +;"), vec![
                incomplete(0, 1),
                invalid(1, 2),
            ]);
        });
    }

    #[test]
    fn test_input_boundaries_invalid_message() {
        r_test(|| {
            let boundaries = input_boundaries("foo )").unwrap();
            assert_eq!(boundaries, vec![InputBoundary::invalid(
                LineRange::new(0, 1),
                String::from("unexpected ')'")
            ),]);
        });
    }
}
