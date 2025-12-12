//
// console_annotate.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

use aether_syntax::RBracedExpressions;
use aether_syntax::RExpressionList;
use aether_syntax::RRoot;
use aether_syntax::RSyntaxNode;
use amalthea::wire::execute_request::CodeLocation;
use biome_line_index::LineIndex;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::SyntaxElement;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use biome_rowan::WalkEvent;
use url::Url;

use crate::dap::dap::Breakpoint;
use crate::dap::dap::BreakpointState;

pub(crate) fn annotate_input(
    code: &str,
    location: CodeLocation,
    breakpoints: Option<&mut [Breakpoint]>,
) -> String {
    // First, inject breakpoints into the original code (before adding line directive).
    // This ensures AST line numbers match the code coordinates we expect.
    let code_with_breakpoints = if let Some(breakpoints) = breakpoints {
        let line_index = LineIndex::new(code);
        inject_breakpoints(code, location.clone(), breakpoints, &line_index)
    } else {
        code.to_string()
    };

    // Now add the line directive to the (possibly modified) code
    let node = aether_parser::parse(&code_with_breakpoints, Default::default()).tree();
    let Some(first_token) = node.syntax().first_token() else {
        return code_with_breakpoints;
    };

    let line_directive = format!(
        "#line {line} \"{uri}\"",
        line = location.start.line + 1,
        uri = location.uri
    );

    // Leading whitespace to ensure that R starts parsing expressions from
    // the expected `character` offset.
    let leading_padding = " ".repeat(location.start.character as usize);

    // Collect existing leading trivia as (kind, text) tuples
    let existing_trivia: Vec<_> = first_token
        .leading_trivia()
        .pieces()
        .map(|piece| (piece.kind(), piece.text().to_string()))
        .collect();

    // Create new trivia with line directive prepended
    let new_trivia: Vec<_> = vec![
        (
            biome_rowan::TriviaPieceKind::SingleLineComment,
            line_directive.to_string(),
        ),
        (biome_rowan::TriviaPieceKind::Newline, "\n".to_string()),
        (
            biome_rowan::TriviaPieceKind::Whitespace,
            leading_padding.to_string(),
        ),
    ]
    .into_iter()
    .chain(existing_trivia.into_iter())
    .collect();

    let new_first_token =
        first_token.with_leading_trivia(new_trivia.iter().map(|(k, t)| (*k, t.as_str())));

    let Some(new_node) = node
        .syntax()
        .clone()
        .replace_child(first_token.into(), new_first_token.into())
    else {
        return code_with_breakpoints;
    };

    new_node.to_string()
}

#[allow(dead_code)]
pub(crate) fn inject_breakpoints(
    code: &str,
    location: CodeLocation,
    breakpoints: &mut [Breakpoint],
    line_index: &LineIndex,
) -> String {
    let root = aether_parser::parse(code, Default::default()).tree();

    // The offset between document coordinates and code coordinates.
    // Breakpoints are in document coordinates, but AST nodes are in code coordinates.
    let line_offset = location.start.line;

    // Filter breakpoints to only those within the source's valid range
    let breakpoints: Vec<_> = breakpoints
        .iter_mut()
        .filter(|bp| bp.line >= location.start.line && bp.line <= location.end.line)
        .collect();

    if breakpoints.is_empty() {
        return code.into();
    }

    // Phase 1: Find breakpoint anchors
    let anchors = find_breakpoint_anchors(
        root.syntax(),
        breakpoints,
        &location.uri,
        line_index,
        line_offset,
    );

    if anchors.is_empty() {
        return code.into();
    }

    // Phase 2: Inject breakpoints
    inject_breakpoint_calls(root.syntax(), anchors, &location.uri, line_offset)
}

struct BreakpointAnchor {
    breakpoint_id: i64,
    /// The line in code coordinates (0-based within parsed code)
    code_line: u32,
}

fn find_breakpoint_anchors(
    root: &RSyntaxNode,
    mut breakpoints: Vec<&mut Breakpoint>,
    uri: &Url,
    line_index: &LineIndex,
    line_offset: u32,
) -> Vec<BreakpointAnchor> {
    // Sort breakpoints by line ascending
    breakpoints.sort_by_key(|bp| bp.line);

    let mut anchors = Vec::new();
    let mut bp_iter = breakpoints.into_iter().peekable();

    // Start from the root's expression list
    let Some(r) = RRoot::cast(root.clone()) else {
        return anchors;
    };
    let root_list = r.expressions();

    find_anchors_in_list(
        &root_list,
        &mut bp_iter,
        &mut anchors,
        uri,
        line_index,
        line_offset,
        true,
    );

    anchors
}

fn find_anchors_in_list<'a>(
    list: &RExpressionList,
    breakpoints: &mut std::iter::Peekable<impl Iterator<Item = &'a mut Breakpoint>>,
    anchors: &mut Vec<BreakpointAnchor>,
    uri: &Url,
    line_index: &LineIndex,
    line_offset: u32,
    is_root: bool,
) {
    let elements: Vec<_> = list.into_iter().collect();

    if elements.is_empty() {
        return;
    }

    let mut i = 0;
    while i < elements.len() {
        let Some(bp) = breakpoints.peek() else {
            return;
        };

        // Convert breakpoint line from document coordinates to code coordinates
        let target_code_line = bp.line - line_offset;
        let current = &elements[i];
        let current_code_line = get_start_line(current.syntax(), line_index);

        // Base case: target line is at or before current element's start
        if target_code_line <= current_code_line {
            let bp = breakpoints.next().unwrap();
            // Update bp.line to the actual document line where the breakpoint is placed
            bp.line = current_code_line + line_offset;
            anchors.push(BreakpointAnchor {
                breakpoint_id: bp.id,
                code_line: current_code_line,
            });
            continue;
        }

        // Check if target is beyond current element
        let next_code_line = if i + 1 < elements.len() {
            Some(get_start_line(elements[i + 1].syntax(), line_index))
        } else {
            None
        };

        // Recursion case: target must be within current element
        if next_code_line.map_or(true, |next| target_code_line < next) {
            // If we're at the last element of a nested list and there's no next element,
            // the target might be beyond this list. Pop back up to let the parent handle it.
            if !is_root && next_code_line.is_none() {
                return;
            }

            // Search within current element for brace lists
            let anchors_before = anchors.len();
            if find_anchor_in_element(
                current.syntax(),
                breakpoints,
                anchors,
                uri,
                line_index,
                line_offset,
            )
            .is_some()
            {
                // A nested brace list was found and processed.
                if anchors.len() > anchors_before {
                    // Anchor(s) placed in nested list. Continue without incrementing
                    // `i` to re-check this element for any remaining breakpoints
                    // (handles multiple breakpoints in same block).
                    continue;
                }
                // The nested list was exhausted without placing an anchor for the
                // current breakpoint. This means the target line is beyond all
                // expressions in the nested list (e.g., on a closing `}` line with
                // no executable code). Mark this breakpoint as invalid.
                let bp = breakpoints.next().unwrap();
                bp.state = BreakpointState::Invalid;
                continue;
            } else {
                // No brace list found, use current element as fallback
                let bp = breakpoints.next().unwrap();
                // Update bp.line to the actual document line where the breakpoint is placed
                bp.line = current_code_line + line_offset;
                anchors.push(BreakpointAnchor {
                    breakpoint_id: bp.id,
                    code_line: current_code_line,
                });
                continue;
            }
        }

        // Continue case: move to next element
        i += 1;
    }
}

fn find_anchor_in_element<'a>(
    element: &RSyntaxNode,
    breakpoints: &mut std::iter::Peekable<impl Iterator<Item = &'a mut Breakpoint>>,
    anchors: &mut Vec<BreakpointAnchor>,
    uri: &Url,
    line_index: &LineIndex,
    line_offset: u32,
) -> Option<()> {
    use biome_rowan::WalkEvent;

    // Search for brace lists in descendants
    for event in element.preorder() {
        let node = match event {
            WalkEvent::Enter(n) => n,
            WalkEvent::Leave(_) => continue,
        };

        if let Some(braced) = RBracedExpressions::cast(node) {
            let expr_list = braced.expressions();
            if !expr_list.is_empty() {
                // Found a non-empty brace list, recurse into it
                find_anchors_in_list(
                    &expr_list,
                    breakpoints,
                    anchors,
                    uri,
                    line_index,
                    line_offset,
                    false,
                );
                return Some(());
            }
        }
    }

    None
}

fn inject_breakpoint_calls(
    root: &RSyntaxNode,
    mut anchors: Vec<BreakpointAnchor>,
    uri: &Url,
    line_offset: u32,
) -> String {
    if anchors.is_empty() {
        return root.to_string();
    }

    // Sort anchors by line DESCENDING so we modify from bottom to top.
    // This preserves line numbers for earlier breakpoints.
    anchors.sort_by_key(|a| std::cmp::Reverse(a.code_line));

    let mut source = root.to_string();

    // Process each breakpoint independently by re-parsing after each injection
    for anchor_info in anchors {
        // Re-parse the current source
        let parse_result = aether_parser::parse(&source, Default::default());
        let root = parse_result.tree();
        let new_line_index = LineIndex::new(&source);

        // Find the anchor node at the target line (using code coordinates)
        let Some(new_anchor) =
            find_node_at_line(root.syntax(), anchor_info.code_line, &new_line_index)
        else {
            continue;
        };

        // Get the parent list and find the anchor's index
        let Some(parent) = new_anchor.parent() else {
            continue;
        };

        let parent_children: Vec<_> = parent.children().collect();
        let Some(index) = parent_children
            .iter()
            .position(|child| child == &new_anchor)
        else {
            continue;
        };

        // Create the breakpoint call and modified anchor
        // Line directive uses document coordinates (code_line + line_offset)
        let breakpoint_call = create_breakpoint_call(uri, anchor_info.breakpoint_id);
        let doc_line = anchor_info.code_line + line_offset;
        let modified_anchor = add_line_directive_to_node(&new_anchor, doc_line, uri);

        // Inject the breakpoint by splicing
        let modified_parent = parent.clone().splice_slots(index..=index, [
            Some(SyntaxElement::Node(breakpoint_call)),
            Some(SyntaxElement::Node(modified_anchor)),
        ]);

        // Propagate the change to the root
        let new_root = propagate_change_to_root(&parent, modified_parent);

        // Update source for next iteration
        source = new_root.to_string();
    }

    source
}

/// Find a node at the specified line in the AST.
/// Returns the first direct child of a list (program or brace list) that starts at or after the target line.
fn find_node_at_line(
    root: &RSyntaxNode,
    target_line: u32,
    line_index: &LineIndex,
) -> Option<RSyntaxNode> {
    // We need to find expression lists and check their children
    for event in root.preorder() {
        let node = match event {
            WalkEvent::Enter(n) => n,
            WalkEvent::Leave(_) => continue,
        };

        // Check if this is a root or brace expression list
        let expr_list = if let Some(r) = RRoot::cast(node.clone()) {
            r.expressions()
        } else if let Some(braced) = RBracedExpressions::cast(node.clone()) {
            braced.expressions()
        } else {
            continue;
        };

        // Check each child of this list
        for expr in expr_list.into_iter() {
            let child_line = get_start_line(expr.syntax(), line_index);
            if child_line == target_line {
                return Some(expr.into_syntax());
            }
        }
    }

    None
}

/// Propagate a node replacement up to the root of the tree.
fn propagate_change_to_root(original: &RSyntaxNode, replacement: RSyntaxNode) -> RSyntaxNode {
    let mut current_original = original.clone();
    let mut current_replacement = replacement;

    while let Some(parent) = current_original.parent() {
        let new_parent = parent
            .clone()
            .replace_child(
                current_original.clone().into(),
                current_replacement.clone().into(),
            )
            .expect("Failed to replace child");

        current_original = parent;
        current_replacement = new_parent;
    }

    current_replacement
}

fn get_start_line(node: &RSyntaxNode, line_index: &LineIndex) -> u32 {
    let text_range: TextRange = node.text_trimmed_range();
    let offset: TextSize = text_range.start();
    line_index.line_col(offset).map(|lc| lc.line).unwrap_or(0)
}

fn create_breakpoint_call(uri: &Url, id: i64) -> RSyntaxNode {
    // NOTE: If you use `base::browser()` here in an attempt to prevent masking
    // issues in case someone redefined `browser()`, you'll cause the function
    // in which the breakpoint is injected to be bytecode-compiled. This is a
    // limitation/bug of https://github.com/r-devel/r-svn/blob/e2aae817/src/library/compiler/R/cmp.R#L1273-L1290
    let code = format!("\nbase::.ark_breakpoint(browser(), \"{uri}\", \"{id}\")\n");
    aether_parser::parse(&code, Default::default()).syntax()
}

fn add_line_directive_to_node(node: &RSyntaxNode, line: u32, uri: &Url) -> RSyntaxNode {
    let Some(first_token) = node.first_token() else {
        return node.clone();
    };

    let line_directive = format!("#line {line} \"{uri}\"", line = line + 1);

    // Collect existing leading trivia, but skip only the first newline to avoid double blank lines
    let existing_trivia: Vec<_> = first_token
        .leading_trivia()
        .pieces()
        .enumerate()
        .filter_map(|(i, piece)| {
            // Skip only the very first newline
            if i == 0 && piece.kind() == biome_rowan::TriviaPieceKind::Newline {
                None
            } else {
                Some((piece.kind(), piece.text().to_string()))
            }
        })
        .collect();

    // Insert line directive before the final whitespace (indentation) if present.
    // This preserves indentation: `[\n, \n, ws]` becomes `[\n, \n, directive, \n, ws]`
    // rather than `[\n, \n, ws, directive, \n]` which would break indentation.
    let new_trivia: Vec<_> = if existing_trivia.last().map_or(false, |(k, _)| {
        *k == biome_rowan::TriviaPieceKind::Whitespace
    }) {
        let (init, last) = existing_trivia.split_at(existing_trivia.len() - 1);
        init.iter()
            .cloned()
            .chain(vec![
                (
                    biome_rowan::TriviaPieceKind::SingleLineComment,
                    line_directive,
                ),
                (biome_rowan::TriviaPieceKind::Newline, "\n".to_string()),
            ])
            .chain(last.iter().cloned())
            .collect()
    } else {
        existing_trivia
            .into_iter()
            .chain(vec![
                (
                    biome_rowan::TriviaPieceKind::SingleLineComment,
                    line_directive,
                ),
                (biome_rowan::TriviaPieceKind::Newline, "\n".to_string()),
            ])
            .collect()
    };

    let new_first_token =
        first_token.with_leading_trivia(new_trivia.iter().map(|(k, t)| (*k, t.as_str())));

    node.clone()
        .replace_child(first_token.into(), new_first_token.into())
        .unwrap_or_else(|| node.clone())
}

#[cfg(test)]
mod tests {
    use amalthea::wire::execute_request::CodeLocation;
    use amalthea::wire::execute_request::Position;
    use url::Url;

    use super::*;

    fn make_location(line: u32, character: u32) -> CodeLocation {
        CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position { line, character },
            end: Position { line, character },
        }
    }

    #[test]
    fn test_annotate_input_basic() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_shifted_line() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(10, 0);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_shifted_character() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(0, 5);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_shifted_line_and_character() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(10, 5);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_existing_whitespace() {
        let code = "  x <- 1\n  y <- 2";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_existing_whitespace_shifted() {
        let code = "  x <- 1\n  y <- 2";
        let location = make_location(0, 2);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_existing_comment() {
        let code = "# comment\nx <- 1";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_empty_code() {
        let code = "";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_breakpoint() {
        // Test the full annotate_input path with breakpoints.
        // This ensures breakpoints are injected correctly before the line directive is added.
        let code = "0\n1\n2";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 3,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 1,
            },
        };
        // Breakpoint at document line 4 (code line 1, i.e., `1`)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 4,
            state: BreakpointState::Unverified,
        }];

        let result = annotate_input(code, location, Some(&mut breakpoints));
        insta::assert_snapshot!(result);

        // Breakpoint line should remain in document coordinates
        assert_eq!(breakpoints[0].line, 4);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_single_line() {
        let code = "x <- 1\ny <- 2\nz <- 3";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 1,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
    }

    #[test]
    fn test_inject_breakpoints_multiple() {
        let code = "x <- 1\ny <- 2\nz <- 3\nw <- 4";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 1,
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 3,
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid)); // Valid location
    }

    #[test]
    fn test_inject_breakpoints_in_brace_list() {
        let code = "f <- function() {\n  x <- 1\n  y <- 2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
    }

    #[test]
    fn test_inject_breakpoints_out_of_range() {
        let code = "x <- 1\ny <- 2";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 1,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 10,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        // Should return unchanged code
        assert_eq!(result, code);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
    }

    #[test]
    fn test_inject_breakpoints_multiple_lists() {
        // This test has breakpoints in different parent lists:
        // - One in the root list
        // - One in a nested brace list
        // This may expose issues with the current propagate_change_to_root approach
        let code = "x <- 1\nf <- function() {\n  y <- 2\n  z <- 3\n}\nw <- 4";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 2,
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 5,
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid)); // Valid location
    }

    #[test]
    fn test_inject_breakpoints_with_blank_line() {
        // Test that blank lines before an anchor are preserved
        let code = "x <- 1\n\n\ny <- 2";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 3,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
    }

    #[test]
    fn test_inject_breakpoints_on_closing_brace() {
        // Breakpoint on a line with only `}` should be left unverified
        // (no executable code there)
        let code = "f <- function() {\n  x <- 1\n}\ny <- 2";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2, // The `}` line
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        // Should return unchanged code since breakpoint is invalid
        assert_eq!(result, code);
        // Marked as invalid
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_on_closing_brace_with_valid_breakpoint() {
        // One breakpoint on `}` (invalid) and one on valid code
        let code = "f <- function() {\n  x <- 1\n}\ny <- 2";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 2, // The `}` line - invalid
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 3, // `y <- 2` - valid
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);

        // Invalid location
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));

        assert!(!matches!(breakpoints[1].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_before_within_after_nested() {
        // Comprehensive test with breakpoints:
        // - Before nested list (line 0: `x <- 1`)
        // - Within nested list (line 2: `y <- 2`)
        // - After nested list (line 5: `w <- 4`)
        let code = "x <- 1\nf <- function() {\n  y <- 2\n  z <- 3\n}\nw <- 4";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 0, // `x <- 1` - before nested
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 2, // `y <- 2` - within nested
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 3,
                line: 5, // `w <- 4` - after nested
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
        assert!(!matches!(breakpoints[2].state, BreakpointState::Verified));
        assert!(!matches!(breakpoints[2].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_with_line_offset() {
        // Test that breakpoints work correctly when the code starts at a non-zero line
        // in the document. This simulates executing a selection from the middle of a file.
        //
        // The code represents lines 10-12 of the original document:
        // Line 10: x <- 1
        // Line 11: y <- 2
        // Line 12: z <- 3
        let code = "x <- 1\ny <- 2\nz <- 3";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 10,
                character: 0,
            },
            end: Position {
                line: 12,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);

        // Breakpoint at document line 11 (which is code line 1, i.e., `y <- 2`)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 11,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);

        // The breakpoint line should remain in document coordinates
        assert_eq!(breakpoints[0].line, 11);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_with_line_offset_nested() {
        // Test with line offset and nested braces
        let code = "f <- function() {\n  x <- 1\n  y <- 2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 20,
                character: 0,
            },
            end: Position {
                line: 23,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        // Breakpoint at document line 22 (code line 2, i.e., `y <- 2`)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 22,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index);
        insta::assert_snapshot!(result);

        // The breakpoint line should remain in document coordinates
        assert_eq!(breakpoints[0].line, 22);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }
}
