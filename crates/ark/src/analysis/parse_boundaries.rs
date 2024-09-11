//
// parse_boundaries.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use harp::ParseResult;

use crate::lsp::offset::ArkPoint;
use crate::lsp::offset::ArkRange;

#[derive(Debug)]
pub struct ParseBoundaries {
    pub complete: Vec<ArkRange>,
    pub incomplete: Option<ArkRange>,
    pub error: Option<ArkRange>,
}

pub fn parse_boundaries(text: &str) -> anyhow::Result<ParseBoundaries> {
    let (status, parse_data) = harp::parse_with_parse_data(text)?;
    let top_level = parse_data.filter_top_level();

    let ranges: Vec<ArkRange> = top_level
        .nodes
        .iter()
        .map(|n| n.as_point_range())
        .map(|r| point_range_as_ark_range(r))
        .collect();

    let boundaries = ParseBoundaries {
        complete: ranges,
        incomplete: None,
        error: None,
    };

    Ok(boundaries)
}

fn point_range_as_ark_range(range: std::ops::Range<(usize, usize)>) -> ArkRange {
    ArkRange {
        start: ArkPoint {
            row: range.start.0,
            column: range.start.1,
        },
        end: ArkPoint {
            row: range.end.0,
            column: range.end.1,
        },
    }
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
