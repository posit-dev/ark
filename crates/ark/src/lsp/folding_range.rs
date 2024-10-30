use tower_lsp::lsp_types::FoldingRange;
use tower_lsp::lsp_types::FoldingRangeKind;

use crate::lsp::documents::Document;
use crate::lsp::log_info;
use crate::lsp::symbols::parse_comment_as_section;

/// Detects and returns folding ranges for comment sections and curly-bracketed blocks
pub fn folding_range(document: &Document) -> anyhow::Result<Vec<FoldingRange>> {
    let mut folding_ranges: Vec<FoldingRange> = Vec::new();
    let text = &document.contents; // Assuming `contents()` gives the text of the document
    let mut line_iter = text.lines().enumerate().peekable();

    let mut comment_stack: Vec<(usize, usize)> = Vec::new(); // a stack of (level, start_line) tuples

    while let Some((line_idx, line)) = line_iter.next() {
        let line_text = line.to_string();
        (folding_ranges, comment_stack) =
            comment_processor(folding_ranges, comment_stack, line_idx, &line_text);
        log_info!("line_idx: {:#?} line_text: {:#?}", line_idx, line_text);
    }

    // TODO: End line handling

    Ok(folding_ranges)
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

    if comment_stack.is_empty() {
        comment_stack.push((level, line_idx));
        return (folding_ranges, comment_stack); // return if the stack is empty
    }

    while let Some((last_level, _)) = comment_stack.last() {
        if *last_level < level {
            comment_stack.push((level, line_idx));
            break;
        } else if *last_level == level {
            folding_ranges.push(comment_range(comment_stack.last().unwrap().1, line_idx - 1));
            comment_stack.pop();
            comment_stack.push((level, line_idx));
            break;
        } else {
            folding_ranges.push(comment_range(comment_stack.last().unwrap().1, line_idx - 1));
            comment_stack.pop();
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
