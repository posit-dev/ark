//
// folding_range.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;
use tower_lsp::lsp_types::FoldingRange;
use tower_lsp::lsp_types::FoldingRangeKind;

use super::symbols::parse_comment_as_section;
use crate::lsp::documents::Document;

pub fn folding_range(document: &Document) -> anyhow::Result<Vec<FoldingRange>> {
    let mut folding_ranges: Vec<FoldingRange> = Vec::new();

    // Activate the parser
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .unwrap();

    let ast = parser.parse(&document.contents.to_string(), None).unwrap();

    if ast.root_node().has_error() {
        tracing::error!("Folding range service: Parse error");
        return Err(anyhow::anyhow!("Parse error"));
    }

    // Traverse the AST
    let mut cursor = ast.root_node().walk();
    parse_ts_node(
        &mut cursor,
        0,
        &mut folding_ranges,
        document,
        &mut vec![Vec::new()],
        &mut None,
        &mut None,
    );

    Ok(folding_ranges)
}

fn parse_ts_node(
    cursor: &mut tree_sitter::TreeCursor,
    _depth: usize,
    folding_ranges: &mut Vec<FoldingRange>,
    document: &Document,
    comment_stack: &mut Vec<Vec<(usize, usize)>>,
    region_marker: &mut Option<usize>,
    cell_marker: &mut Option<usize>,
) {
    let node = cursor.node();
    let _field_name = match cursor.field_name() {
        Some(name) => format!("{name}: "),
        None => String::new(),
    };

    let start = node.start_position();
    let end = node.end_position();
    let node_type = node.kind();

    match node_type {
        "parameters" | "arguments" | "braced_expression" => {
            // Ignore same line folding
            if start.row == end.row {
                return;
            }
            let folding_range = bracket_range(
                start.row,
                start.column + 1, // Start after the opening delimiter
                end.row,
                end.column - 1,
                count_leading_whitespaces(document, end.row),
            );
            folding_ranges.push(folding_range);
        },
        "comment" => {
            // Only process standalone comment
            if count_leading_whitespaces(document, start.row) != start.column {
                return;
            }

            // Nested comment section handling
            let comment_line = get_line_text(document, start.row, None, None);

            nested_processor(
                document,
                comment_stack,
                folding_ranges,
                start.row,
                &comment_line,
            );
            region_processor(folding_ranges, region_marker, start.row, &comment_line);
            cell_processor(folding_ranges, cell_marker, start.row, &comment_line);
        },
        _ => (),
    }

    if cursor.goto_first_child() {
        // create node child stacks
        // This is a stack of stacks for each bracket level, within each stack is a vector of (level, start_line) tuples
        let mut child_comment_stack: Vec<Vec<(usize, usize)>> = vec![Vec::new()];
        let mut child_region_marker: Option<usize> = None;
        let mut child_cell_marker: Option<usize> = None;

        // recursive loop
        loop {
            parse_ts_node(
                cursor,
                _depth + 1,
                folding_ranges,
                document,
                &mut child_comment_stack,
                &mut child_region_marker,
                &mut child_cell_marker,
            );
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        // End of node handling
        end_node_handler(
            document,
            folding_ranges,
            end.row,
            &mut child_comment_stack,
            &mut child_region_marker,
            &mut child_cell_marker,
        );

        cursor.goto_parent();
    }
}

// Function to create a folding range that specifically deals with bracket ending
fn bracket_range(
    start_line: usize,
    start_char: usize,
    end_line: usize,
    end_char: usize,
    white_space_count: usize,
) -> FoldingRange {
    let mut end_line: u32 = end_line.try_into().unwrap();
    let mut end_char: Option<u32> = Some(end_char.try_into().unwrap());

    let adjusted_end_char = end_char.and_then(|val| val.checked_sub(white_space_count as u32));

    match adjusted_end_char {
        Some(0) => {
            end_line -= 1;
            end_char = None;
        },
        Some(_) => {
            if let Some(ref mut value) = end_char {
                *value -= 1;
            }
        },
        None => {
            tracing::error!(
                "Folding Range (bracket_range): adjusted_end_char should not be None here"
            );
        },
    }

    FoldingRange {
        start_line: start_line.try_into().unwrap(),
        start_character: Some(start_char as u32),
        end_line,
        end_character: end_char,
        kind: Some(FoldingRangeKind::Region),
        collapsed_text: None,
    }
}

fn comment_range(start_line: usize, end_line: usize) -> FoldingRange {
    FoldingRange {
        start_line: start_line.try_into().unwrap(),
        start_character: None,
        end_line: end_line.try_into().unwrap(),
        end_character: None,
        kind: Some(FoldingRangeKind::Region),
        collapsed_text: None,
    }
}

fn get_line_text(
    document: &Document,
    line_num: usize,
    start_char: Option<usize>,
    end_char: Option<usize>,
) -> String {
    let text = &document.contents;
    // Split the text into lines
    let lines: Vec<&str> = text.lines().filter_map(|line| line.as_str()).collect();

    // Ensure the start_line is within bounds
    if line_num >= lines.len() {
        return String::new(); // Return an empty string if out of bounds
    }

    // Get the line corresponding to start_line
    let line = lines[line_num];

    // Determine the start and end character indices
    let start_idx = start_char.unwrap_or(0); // Default to 0 if None
    let end_idx = end_char.unwrap_or(line.len()); // Default to the line's length if None

    // Ensure indices are within bounds for the line
    let start_idx = start_idx.min(line.len());
    let end_idx = end_idx.min(line.len());

    // Extract the substring and return it
    line[start_idx..end_idx].to_string()
}

fn find_last_non_empty_line(document: &Document, start_line: usize, end_line: usize) -> usize {
    for idx in (start_line..=end_line).rev() {
        if !get_line_text(document, idx, None, None).trim().is_empty() {
            return idx;
        }
    }
    start_line
}

fn count_leading_whitespaces(document: &Document, line_num: usize) -> usize {
    let line_text = get_line_text(document, line_num, None, None);
    line_text.chars().take_while(|c| c.is_whitespace()).count()
}

pub static RE_COMMENT_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(#+)\s*(.*?)\s*[#=-]{4,}\s*$").unwrap());

fn nested_processor(
    document: &Document,
    comment_stack: &mut Vec<Vec<(usize, usize)>>,
    folding_ranges: &mut Vec<FoldingRange>,
    line_num: usize,
    comment_line: &str,
) {
    let Some((level, _title)) = parse_comment_as_section(comment_line) else {
        return; // return if the line is not a comment section
    };
    if comment_stack.is_empty() {
        tracing::error!(
            "Folding Range: comment_stack should always contain at least one element here"
        );
        return;
    }
    loop {
        if comment_stack.last().unwrap().is_empty() {
            comment_stack.last_mut().unwrap().push((level, line_num));
            return; // return if the stack is empty
        }

        let Some((last_level, _)) = comment_stack.last().unwrap().last() else {
            tracing::error!("Folding Range: comment_stacks should not be empty here");
            return;
        };
        match last_level.cmp(&level) {
            Ordering::Less => {
                comment_stack.last_mut().unwrap().push((level, line_num));
                break;
            },
            Ordering::Equal => {
                let start_line = comment_stack.last().unwrap().last().unwrap().1;
                folding_ranges.push(comment_range(
                    start_line,
                    find_last_non_empty_line(document, start_line, line_num - 1),
                ));
                comment_stack.last_mut().unwrap().pop();
                comment_stack.last_mut().unwrap().push((level, line_num));
                break;
            },
            Ordering::Greater => {
                let start_line = comment_stack.last().unwrap().last().unwrap().1;
                folding_ranges.push(comment_range(
                    start_line,
                    find_last_non_empty_line(document, start_line, line_num - 1),
                ));
                comment_stack.last_mut().unwrap().pop(); // Safe: the loop exits early if the stack becomes empty
            },
        }
    }
}

/// Enum representing the type of region marker
#[derive(Debug, PartialEq, Eq)]
enum RegionType {
    Start,
    End,
}

fn region_processor(
    folding_ranges: &mut Vec<FoldingRange>,
    region_marker: &mut Option<usize>,
    line_idx: usize,
    line_text: &str,
) {
    let Some(region_type) = parse_region_type(line_text) else {
        return; // return if the line is not a region section
    };
    match region_type {
        RegionType::Start => {
            region_marker.replace(line_idx);
        },
        RegionType::End => {
            if let Some(region_start) = region_marker {
                let folding_range = comment_range(*region_start, line_idx);
                folding_ranges.push(folding_range);
                *region_marker = None;
            }
        },
    }
}

fn parse_region_type(line_text: &str) -> Option<RegionType> {
    let region_start = Regex::new(r"^\s*#+ #region\b").unwrap();
    let region_end = Regex::new(r"^\s*#+ #endregion\b").unwrap();

    if region_start.is_match(line_text) {
        Some(RegionType::Start)
    } else if region_end.is_match(line_text) {
        Some(RegionType::End)
    } else {
        None
    }
}

fn cell_processor(
    // Almost identical to region_processor
    folding_ranges: &mut Vec<FoldingRange>,
    cell_marker: &mut Option<usize>,
    line_idx: usize,
    line_text: &str,
) {
    let cell_pattern: Regex = Regex::new(r"^#\s*(%%|\+)(.*)").unwrap();

    if !cell_pattern.is_match(line_text) {
    } else {
        let Some(start_line) = cell_marker else {
            cell_marker.replace(line_idx);
            return;
        };

        let folding_range = comment_range(*start_line, line_idx - 1);
        folding_ranges.push(folding_range);
        cell_marker.replace(line_idx);
    }
}

fn end_node_handler(
    document: &Document,
    folding_ranges: &mut Vec<FoldingRange>,
    line_idx: usize,
    comment_stack: &mut Vec<Vec<(usize, usize)>>,
    region_marker: &mut Option<usize>,
    cell_marker: &mut Option<usize>,
) {
    // Nested comment handling
    // Iterate over the last element of the comment stack and add it to the folding ranges by using the comment_range function
    if let Some(last_section) = comment_stack.last() {
        // Iterate over each (start level, start line) in the last section
        for &(_level, start_line) in last_section.iter() {
            // Add a new folding range for each range in the last section
            let folding_range = comment_range(
                start_line,
                find_last_non_empty_line(document, start_line, line_idx - 1),
            );

            folding_ranges.push(folding_range);
        }
    }
    // Remove the last element from the comment stack after processing
    comment_stack.pop();

    // Unclosed region handling
    if let Some(region_start) = region_marker {
        let folding_range = comment_range(*region_start, line_idx - 1);
        folding_ranges.push(folding_range);
        *region_marker = None;
    }

    // End cell Handling
    if let Some(cell_start) = cell_marker {
        // For the last cell, include the current line in the folding range
        let folding_range = comment_range(
            *cell_start,
            find_last_non_empty_line(document, *cell_start, line_idx),
        );
        folding_ranges.push(folding_range);
        *cell_marker = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::documents::Document;

    fn test_folding_range(code: &str) -> Vec<FoldingRange> {
        let doc = Document::new(code, None);
        // Sort ranges for more consistent testing
        sorted_ranges(folding_range(&doc).unwrap())
    }

    fn sorted_ranges(mut ranges: Vec<FoldingRange>) -> Vec<FoldingRange> {
        ranges.sort_by(|a, b| {
            a.start_line
                .cmp(&b.start_line)
                .then(a.end_line.cmp(&b.end_line))
        });
        ranges
    }

    #[test]
    fn test_parse_region_type() {
        // Not regions
        assert_eq!(parse_region_type("# # region"), None);
        assert_eq!(parse_region_type("# # endregion"), None);
        assert_eq!(parse_region_type("# # not a region"), None);
        assert_eq!(parse_region_type("# #regionsomething"), None);
        assert_eq!(parse_region_type("# #endregionsomething"), None);

        // Valid regions
        assert_eq!(parse_region_type("# #region"), Some(RegionType::Start));
        assert_eq!(parse_region_type("## #region  "), Some(RegionType::Start));
        assert_eq!(
            parse_region_type("# #region my special area"),
            Some(RegionType::Start)
        );

        assert_eq!(parse_region_type("# #endregion"), Some(RegionType::End));
        assert_eq!(parse_region_type("## #endregion  "), Some(RegionType::End));
        assert_eq!(
            parse_region_type("# #endregion end of my special area"),
            Some(RegionType::End)
        );
    }

    #[test]
    fn test_folding_section_comments_basic() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
# First section ----
a
b

# Second section ----
c
d"
        ));
    }

    #[test]
    fn test_folding_nested_section_comments() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
# Level 1 ----
a

## Level 2 ----
b

### Level 3 ----
c

## Another Level 2 ----
d

# Back to Level 1 ----
e"
        ));
    }

    #[test]
    fn test_folding_empty_sections() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
# Empty section ----

# Another empty section ----

# Section with content ----
a"
        ));
    }

    // Test for VS Code region markers
    #[test]
    fn test_folding_regions() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
# #region Important code
a
b
c
# #endregion

# #region Another section
d
# #endregion"
        ));
    }

    // Test for cells (like Jupyter notebook cells)
    #[test]
    fn test_folding_cells() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
# %% First cell
a
b

# %% Second cell
c

# %% Third cell
d"
        ));
    }

    // Test for bracket-based folding
    #[test]
    fn test_folding_brackets() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
function() {
  if (condition) {
    a
  } else {
    b
  }
}

list <- list(
  a = 1,
  b = 2,
  c = 3
)"
        ));
    }

    // Test for mixed folding strategies
    #[test]
    fn test_folding_mixed() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
# First section ----
function() {
  # #region nested region
  a
  # #endregion
}

## Subsection ----
# %% Cell in subsection
b

# Another section ----
c"
        ));
    }

    // Test for edge case with single-line braces
    #[test]
    fn test_folding_single_line_braces() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
function() { a }

function() {
  b
}"
        ));
    }

    // Test for nested, complex code structures
    #[test]
    fn test_folding_complex_nested() {
        insta::assert_debug_snapshot!(test_folding_range(
            "
# Complex example ----
function(a, b, c) {
  # #region inner calculations
  x <- a + b
  y <- b + c

  if (x > y) {
    # %% cell inside function
    result <- x * y
  } else {
    result <- x / y
  }
  # #endregion

  result
}

## Subsection ----
# This is a regular comment, not a section or region"
        ));
    }

    // Test for unterminated structures
    #[test]
    fn test_folding_unterminated() {
        // Add try_unwrap to handle the expected error from the parser
        let doc = Document::new(
            "
# #region without end

# %% cell without another cell

function() {
  # Unclosed function
",
            None,
        );

        // Handle the expected parse error
        match folding_range(&doc) {
            Ok(ranges) => insta::assert_debug_snapshot!(sorted_ranges(ranges)),
            Err(e) => insta::assert_debug_snapshot!(format!("Expected error: {}", e)),
        }
    }

    // Test for correct last non-empty line detection
    #[test]
    fn test_find_last_non_empty_line() {
        let doc = Document::new("\nline1\nline2\n\nline3\n", None);

        assert_eq!(find_last_non_empty_line(&doc, 1, 5), 4);
        assert_eq!(find_last_non_empty_line(&doc, 1, 2), 2);
        assert_eq!(find_last_non_empty_line(&doc, 3, 5), 4);
        assert_eq!(find_last_non_empty_line(&doc, 5, 5), 5);

        // Test with empty document
        let empty_doc = Document::new("\n\n", None);
        assert_eq!(find_last_non_empty_line(&empty_doc, 1, 2), 1);
    }

    // Test for whitespace counting
    #[test]
    fn test_count_leading_whitespaces() {
        let doc = Document::new(
            "no spaces
  two spaces
    four spaces
\ttab char",
            None,
        );

        assert_eq!(count_leading_whitespaces(&doc, 0), 0);
        assert_eq!(count_leading_whitespaces(&doc, 1), 2);
        assert_eq!(count_leading_whitespaces(&doc, 2), 4);
        assert_eq!(count_leading_whitespaces(&doc, 3), 1); // Tab counts as 1 char
    }
}
