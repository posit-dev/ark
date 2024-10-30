use tower_lsp::lsp_types::FoldingRange;
use tower_lsp::lsp_types::FoldingRangeKind;

use crate::lsp::documents::Document;
use crate::lsp::log_error;
use crate::lsp::log_info;
use crate::lsp::symbols::parse_comment_as_section;

/// Detects and returns folding ranges for comment sections and curly-bracketed blocks
pub fn folding_range(document: &Document) -> anyhow::Result<Vec<FoldingRange>> {
    let mut folding_ranges: Vec<FoldingRange> = Vec::new();
    let text = &document.contents; // Assuming `contents()` gives the text of the document
    let mut line_iter = text.lines().enumerate().peekable();

    let mut bracket_stack: Vec<(usize, usize)> = Vec::new(); // a stack of (start_line, start_character) tuples
    let mut comment_stack: Vec<(usize, usize)> = Vec::new(); // a stack of (level, start_line) tuples

    while let Some((line_idx, line)) = line_iter.next() {
        let line_text = line.to_string();
        (folding_ranges, bracket_stack) =
            bracket_processor(folding_ranges, bracket_stack, line_idx, &line_text);
        (folding_ranges, comment_stack) =
            comment_processor(folding_ranges, comment_stack, line_idx, &line_text);
        // log_info!("line_idx: {:#?} line_text: {:#?}", line_idx, line_text);
    }

    // TODO: End line handling

    // Log the final folding ranges
    log_info!("folding_ranges: {:#?}", folding_ranges);

    Ok(folding_ranges)
}

fn bracket_processor(
    mut folding_ranges: Vec<FoldingRange>,
    mut bracket_stack: Vec<(usize, usize)>,
    line_idx: usize,
    line_text: &str,
) -> (Vec<FoldingRange>, Vec<(usize, usize)>) {
    // Remove any trailing comments (starting with #) and \n in line_text
    let line_text = line_text.split('#').next().unwrap_or("").trim_end();
    let mut whitespace_count = 0;

    // Iterate over each character in line_text to find the positions of `{` and `}`
    for (char_idx, c) in line_text.char_indices() {
        match c {
            '{' => {
                bracket_stack.push((line_idx, char_idx));
            },
            '}' => {
                // If '}' is found, pop from the bracket_stack if it is not empty
                if let Some((start_line, start_char)) = bracket_stack.pop() {
                    // Count the number of leading whitespace characters

                    // Create a new FoldingRange from the start `{` to the current `}`
                    let folding_range = bracket_range(
                        start_line,
                        start_char,
                        line_idx,
                        char_idx,
                        &whitespace_count,
                    );

                    // Log a copy of the folding range
                    // let folding_range_copy = folding_range.clone();
                    // log_info!("folding_range_copy: {:#?}", folding_range_copy);

                    // Add the folding range to the list
                    folding_ranges.push(folding_range);
                }
            },
            ' ' => whitespace_count += 1,
            _ => {},
        }
    }

    (folding_ranges, bracket_stack)
}

fn bracket_range(
    start_line: usize,
    start_char: usize,
    end_line: usize,
    end_char: usize,
    white_space_count: &usize,
) -> FoldingRange {
    let mut end_line: u32 = end_line.try_into().unwrap();
    let mut end_char: Option<u32> = Some(end_char.try_into().unwrap());

    let adjusted_end_char = end_char.and_then(|val| val.checked_sub(*white_space_count as u32));

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
            log_error!("Folding Range (bracket_range): adjusted_end_char should not be None here");
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

fn comment_processor(
    mut folding_ranges: Vec<FoldingRange>,
    mut comment_stack: Vec<(usize, usize)>,
    line_idx: usize,
    line_text: &str,
) -> (Vec<FoldingRange>, Vec<(usize, usize)>) {
    let Some((level, _title)) = parse_comment_as_section(line_text) else {
        return (folding_ranges, comment_stack); // return if the line is not a comment section
    };

    loop {
        if comment_stack.is_empty() {
            comment_stack.push((level, line_idx));
            return (folding_ranges, comment_stack); // return if the stack is empty
        }

        let Some((last_level, _)) = comment_stack.last() else {
            log_error!("Folding Range: comment_stacks should not be empty here");
            return (folding_ranges, comment_stack);
        };
        if *last_level < level {
            comment_stack.push((level, line_idx));
            break;
        } else if *last_level == level {
            folding_ranges.push(comment_range(comment_stack.last().unwrap().1, line_idx - 1));

            // Log a copy of folding_range
            let folding_range_copy = folding_ranges.last().unwrap().clone();
            log_info!("folding_range_copy: {:#?}", folding_range_copy);

            comment_stack.pop();
            comment_stack.push((level, line_idx));
            break;
        } else {
            folding_ranges.push(comment_range(comment_stack.last().unwrap().1, line_idx - 1));
            comment_stack.pop(); // TODO: Handle case where comment_stack is empty
        }
    }

    // log a copy of comment_stack
    let comment_stack_copy = comment_stack.clone();
    log_info!("comment_stack_copy: {:#?}", comment_stack_copy);

    (folding_ranges, comment_stack)
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
