use anyhow::anyhow;

use crate::lsp::config::IndentStyle;
use crate::lsp::config::IndentationConfig;
use crate::lsp::documents::Document;
use crate::lsp::offset::ArkPoint;
use crate::lsp::offset::ArkRange;
use crate::lsp::offset::ArkTextEdit;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeTypeExt;

/// Provide indentation corrections
///
/// Hooked up to format-on-type for newline characters.
///
/// This is not a full indenter yet. We only provide corrections for the
/// Positron frontend when the VS Code regexp-based indenting rules are not able
/// to indent as expected. For instance we reindent pipeline components to
/// ensure alignment and avoid a staircase effect.
///
/// Once we implement a full formatter, indentation will be provided for any
/// constructs based on the formatter and will be fully consistent with it.
pub fn indent_edit(doc: &Document, line: usize) -> anyhow::Result<Option<Vec<ArkTextEdit>>> {
    let text = &doc.contents;
    let ast = &doc.ast;
    let config = &doc.config.indent;

    // Rope counts from 1, we count from 0
    if line >= text.len_lines() {
        return Err(anyhow!("`line` is OOB"));
    }

    let indent_pos = tree_sitter::Point {
        row: line,
        column: 0,
    };

    let node = ast
        .root_node()
        .find_smallest_spanning_node(indent_pos)
        .unwrap(); // Can only happen if `line` is OOB, which it isn't

    // Get the parent node of the beginning of line
    let mut bol_parent = node;
    while bol_parent.start_position().row == line {
        if let Some(parent) = bol_parent.parent() {
            bol_parent = parent;
        } else {
            break;
        }
    }

    // log::trace!("node: {node:?}");
    // log::trace!("bol_parent: {bol_parent:?}");

    // Iterator over characters following `line`'s indent
    let text_at_indent = || {
        text.chars_at(text.line_to_char(line))
            .skip_while(|c| *c == ' ' || *c == '\t')
    };

    // The indentation of the line the node starts on
    let node_line_indent = |point: tree_sitter::Node| -> usize {
        line_indent(text, point.start_position().row, config).0
    };
    let brace_parent_indent =
        |node: tree_sitter::Node| -> usize { node_line_indent(brace_parent(node)) };

    let brace_indent = |parent: tree_sitter::Node| -> (usize, usize) {
        // If we're looking at a closing delimiter, indent at the parent's
        // beginning of line
        if let Some(c) = text_at_indent().next() {
            if c == '}' {
                return (brace_parent_indent(parent), 0);
            }
            // else fallthrough
        };

        (brace_parent_indent(parent), config.indent_size)
    };

    // Structured in two stages as in Emacs TS rules: first match, then
    // return anchor and indent size. We can add more rules here as needed.
    let (anchor, indent) = match bol_parent {
        // Indentation of top-level expressions. Fixes some problematic
        // outdents:
        // https://github.com/posit-dev/positron/issues/1880
        // https://github.com/posit-dev/positron/issues/2764
        parent if parent.is_program() => (parent.start_position().column, 0),
        parent if parent.is_braced_expression() => brace_indent(parent),

        // Indentation of chained operators (aka pipelines):
        // https://github.com/posit-dev/positron/issues/2707
        parent if parent.is_binary_operator() => {
            let anchor = node
                .ancestors()
                .find(|n| n.parent().map_or(true, |p| !p.is_binary_operator()))
                .unwrap_or(parent); // Should not happen

            (anchor.start_position().column, config.indent_size)
        },
        _ => return Ok(None),
    };

    let new_indent = anchor + indent;
    let (old_indent, old_indent_byte) = line_indent(text, line, config);

    if old_indent == new_indent {
        return Ok(None);
    }

    let new_text = new_line_indent(config, new_indent);

    let beg = ArkPoint {
        row: line,
        column: 0,
    };
    let end = ArkPoint {
        row: line,
        column: old_indent_byte,
    };
    let edit = ArkTextEdit {
        range: ArkRange { start: beg, end },
        new_text,
    };

    Ok(Some(vec![edit]))
}

fn brace_parent(node: tree_sitter::Node) -> tree_sitter::Node {
    let Some(parent) = node.parent() else {
        return node;
    };

    match parent.node_type() {
        crate::treesitter::NodeType::FunctionDefinition => parent,
        crate::treesitter::NodeType::IfStatement => parent,
        crate::treesitter::NodeType::ForStatement => parent,
        crate::treesitter::NodeType::WhileStatement => parent,
        crate::treesitter::NodeType::RepeatStatement => parent,
        _ => node,
    }
}

/// Returns indent as a pair of space size and byte size
pub fn line_indent(text: &ropey::Rope, line: usize, config: &IndentationConfig) -> (usize, usize) {
    let mut byte_indent = 0;
    let mut indent = 0;
    let mut iter = text.chars_at(text.line_to_char(line));

    while let Some(next_char) = iter.next() {
        if next_char == ' ' {
            indent = indent + 1;
            byte_indent = byte_indent + 1;
            continue;
        } else if next_char == '\t' {
            indent = indent + config.tab_width;
            byte_indent = byte_indent + 1;
            continue;
        }
        break;
    }

    (indent, byte_indent)
}

pub fn new_line_indent(config: &IndentationConfig, indent: usize) -> String {
    match config.indent_style {
        IndentStyle::Space => String::from(' ').repeat(indent),
        IndentStyle::Tab => {
            let n_tabs = indent / config.tab_width;
            let n_spaces = indent % config.tab_width;
            String::from('\t').repeat(n_tabs) + &String::from(' ').repeat(n_spaces)
        },
    }
}

#[cfg(test)]
mod tests {
    use harp::assert_match;

    use crate::lsp::config::IndentStyle;
    use crate::lsp::config::IndentationConfig;
    use crate::lsp::documents::Document;
    use crate::lsp::indent::indent_edit;
    use crate::lsp::indent::new_line_indent;
    use crate::lsp::offset::apply_text_edits;

    // NOTE: If we keep adding tests we might want to switch to snapshot tests

    const SPACE_CFG: IndentationConfig = IndentationConfig {
        indent_style: IndentStyle::Space,
        indent_size: 2,
        tab_width: 2,
    };

    fn test_doc(text: &str) -> Document {
        let mut doc = Document::new(text, None);
        doc.config.indent = SPACE_CFG;
        doc
    }

    #[test]
    fn test_line_indent_oob() {
        let doc = test_doc("");
        assert_match!(indent_edit(&doc, 1), Err(_));

        let doc = test_doc("\n");
        assert_match!(indent_edit(&doc, 2), Err(_));
    }

    #[test]
    fn test_line_indent_chains() {
        let mut text = String::from("foo +\n  bar +\n    baz + qux |>\nfoofy()");
        let doc = test_doc(&text);

        // Indenting the first two lines doesn't change the text
        assert_match!(indent_edit(&doc, 0), Ok(None));
        assert_match!(indent_edit(&doc, 1), Ok(None));

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(
            text,
            String::from("foo +\n  bar +\n  baz + qux |>\nfoofy()")
        );

        let edit = indent_edit(&doc, 3).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(
            text,
            String::from("foo +\n  bar +\n  baz + qux |>\n  foofy()")
        );
    }

    #[test]
    fn test_line_indent_chains_trailing_space() {
        let mut text = String::from("foo +\n  bar(\n    x\n  ) +\n    baz\n  ");
        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 4).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, String::from("foo +\n  bar(\n    x\n  ) +\n  baz\n  "));
    }

    #[test]
    fn test_line_indent_chains_outdent() {
        let text = String::from("1 +\n  2\n");
        let doc = test_doc(&text);

        assert_match!(indent_edit(&doc, 2), Ok(None));
    }

    #[test]
    fn test_line_indent_chains_deep() {
        let mut text = String::from("deep()()[] +\n    deep()()[]");
        let expected = String::from("deep()()[] +\n  deep()()[]");
        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, expected);
    }

    #[test]
    fn test_line_indent_chains_deep_newlines() {
        // With newlines in the way
        let mut text = String::from("deep(\n)()[] +\ndeep(\n)()[]");
        let expected = String::from("deep(\n)()[] +\n  deep(\n)()[]");
        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, expected);
    }

    #[test]
    fn test_line_indent_chains_calls() {
        let mut text = String::from("foo() +\n  bar() +\nbaz()");
        let expected = String::from("foo() +\n  bar() +\n  baz()");

        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, expected);

        // Indenting the first two lines doesn't change the text
        let edit = indent_edit(&doc, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&doc, 1).unwrap();
        assert!(edit.is_none());

        let doc = test_doc("foo(\n) +\n  bar");
        let edit = indent_edit(&doc, 0).unwrap();
        assert!(edit.is_none());
    }

    #[test]
    fn test_line_indent_braced_expression() {
        let mut text = String::from("{\nbar\n}");
        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, String::from("{\n  bar\n}"));

        let mut text = String::from("function() {\nbar\n}");
        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, String::from("function() {\n  bar\n}"));
    }

    #[test]
    fn test_line_indent_braced_expression_closing() {
        let mut text = String::from("{\n  }");
        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, String::from("{\n}"));
    }

    #[test]
    fn test_line_indent_braced_expression_multiline() {
        let mut text = String::from("function(\n        ) {\nfoo\n}");
        let doc = test_doc(&text);

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut text).unwrap();
        assert_eq!(text, String::from("function(\n        ) {\n  foo\n}"));
    }

    #[test]
    fn test_new_line_indent() {
        let tab_cfg = IndentationConfig {
            indent_style: IndentStyle::Tab,
            indent_size: 4,
            tab_width: 4,
        };
        let large_tab_cfg = IndentationConfig {
            indent_style: IndentStyle::Tab,
            indent_size: 4,
            tab_width: 8,
        };

        assert_eq!(
            new_line_indent(&SPACE_CFG, 12),
            String::from(' ').repeat(12)
        );

        assert_eq!(new_line_indent(&tab_cfg, 7), String::from("\t   "));
        assert_eq!(new_line_indent(&tab_cfg, 8), String::from("\t\t"));
        assert_eq!(new_line_indent(&tab_cfg, 9), String::from("\t\t "));

        assert_eq!(
            new_line_indent(&large_tab_cfg, 7),
            String::from(' ').repeat(7)
        );
        assert_eq!(new_line_indent(&large_tab_cfg, 8), String::from("\t"));
        assert_eq!(new_line_indent(&large_tab_cfg, 12), String::from("\t    "));
    }

    fn read_text_asset(path: &str) -> String {
        let mut asset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        asset.push("src");
        asset.push(path);
        std::fs::read_to_string(asset).unwrap()
    }

    fn write_asset(path: &str, text: &str) {
        let mut asset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        asset.push("src");
        asset.push(path);
        std::fs::write(asset, text).unwrap();
    }

    #[test]
    fn test_indent_snapshot() {
        let orig = read_text_asset("lsp/snapshots/indent.R");

        let doc = test_doc(&orig);

        let mut text = orig.clone();
        let n_lines = text.matches('\n').count();

        for i in 0..n_lines {
            if let Some(edit) = indent_edit(&doc, i).unwrap() {
                apply_text_edits(edit, &mut text).unwrap();
            }
        }

        write_asset("lsp/snapshots/indent.R", &text);

        if orig != text {
            panic!("Indentation snapshots have changed.\nPlease see git diff.");
        }
    }
}
