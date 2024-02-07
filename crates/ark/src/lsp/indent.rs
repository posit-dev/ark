use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::TextEdit;

use crate::lsp::documents::Document;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

const INDENT_SIZE: usize = 2;

pub(crate) fn indent(
    pos: Position,
    node: tree_sitter::Node,
    doc: &Document,
) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let contents = &doc.contents;

    let (anchor, indent) = match node.parent() {
        Some(parent) if matches!(parent.node_type(), NodeType::BinaryOperator(_)) => {
            let mut anchor = parent;

            while let Some(p) = anchor.parent() {
                if matches!(p.node_type(), NodeType::BinaryOperator(_)) {
                    anchor = p;
                } else {
                    break;
                }
            }
            (anchor, INDENT_SIZE)
        },
        _ => return Ok(None),
    };

    // TODO: Check column type/quantity
    let anchor_pos = anchor.start_position();
    let old_indent = get_line_indent(contents, pos.line as usize);
    let new_indent = anchor_pos.column + indent;

    if old_indent == new_indent {
        return Ok(None);
    }

    let growing = old_indent < new_indent;

    let edit = if growing {
        let new_text = String::from(' ').repeat(new_indent - old_indent);
        let pos = Position {
            line: pos.line,
            character: old_indent as u32,
        };
        TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text,
        }
    } else {
        let pos1 = Position {
            line: pos.line,
            character: new_indent as u32,
        };
        let pos2 = Position {
            line: pos.line,
            character: old_indent as u32,
        };
        TextEdit {
            range: Range {
                start: pos1,
                end: pos2,
            },
            new_text: String::from(""),
        }
    };

    Ok(Some(vec![edit]))
}

fn get_line_indent(contents: &ropey::Rope, line: usize) -> usize {
    let mut indent = 0;
    let mut iter = contents.chars_at(contents.line_to_char(line));

    while let Some(next_char) = iter.next() {
        if next_char == ' ' {
            indent = indent + 1;
            continue;
        } else if next_char == '\t' {
            // TODO: Custom tab size
            indent = indent + INDENT_SIZE;
            continue;
        }
        break;
    }

    indent
}
