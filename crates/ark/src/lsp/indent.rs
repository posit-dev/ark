use anyhow::anyhow;
use tower_lsp::lsp_types::TextEdit;

use crate::lsp::config::IndentStyle;
use crate::lsp::config::IndentationConfig;
use crate::lsp::documents::Document;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeType;
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
pub fn indent_edit(doc: &Document, line: usize) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let text = &doc.contents;
    let ast = &doc.ast;
    let config = &doc.config.indent;

    let line_count = if text.is_empty() {
        1
    } else {
        text.chars().filter(|c| *c == '\n').count() + 1
    };
    if line >= line_count {
        return Err(anyhow!("`line` is OOB"));
    }

    let indent_pos = tree_sitter::Point {
        row: line,
        column: 0,
    };

    let node = ast.root_node().find_smallest_spanning_node(indent_pos);

    // FIXME: Remove this as soon as https://github.com/r-lib/tree-sitter-r/pull/126
    // is merged and we have synced with upstream tree-sitter-r.
    // Due to a tree-sitter-r bug, if there are leading newlines in a document, they are
    // consumed before the `program` node is created, meaning that rows at the beginning
    // of a document before the first token can look OOB and won't be contained by any
    // node. There should be no indent adjustment required in these cases.
    if node.is_none() {
        return Ok(None);
    }

    let node = node.unwrap(); // Can only happen if `line` is OOB, which it isn't

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
        text.lines()
            .nth(line)
            .map(|line_text| line_text.chars().skip_while(|c| *c == ' ' || *c == '\t'))
            .into_iter()
            .flatten()
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

    let (old_indent, old_indent_byte) = line_indent(text, line, config);

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

            (node_line_indent(anchor), config.indent_size)
        },
        _ => {
            // Find nearest containing braced expression or top-level node. We'll use
            // that to prevent ever indenting past these in unhandled cases for which we
            // don't have rules yet: https://github.com/posit-dev/positron/issues/1683

            // First climb one level if cursor is in front of a `{` character.
            // In that case `node` is the `{` token which is an immediate child
            // of the containing `{` expression. We want to indent that braced
            // expression relative to the next enclosing `{` expression.
            let mut node = node;
            if let Some(c) = text_at_indent().next() {
                if c == '{' {
                    if let Some(parent) = node.parent() {
                        node = parent;
                    }
                }
            }

            // Find nearest enclosing brace. If there is none, just use current indentation.
            let Some(enclosing_brace) = find_enclosing_brace(node) else {
                return Ok(None);
            };
            let (anchor, indent) = brace_indent(enclosing_brace);

            // Only correct if we're too far on the left, past the indentation
            // implied by the enclosing brace
            let min_indent = anchor + indent;
            if old_indent >= min_indent {
                return Ok(None);
            }

            (anchor, indent)
        },
    };

    let new_indent = anchor + indent;

    if old_indent == new_indent {
        return Ok(None);
    }

    let new_text = new_line_indent(config, new_indent);

    let range = tree_sitter::Range {
        start_byte: 0, // Not used by lsp_range_from_tree_sitter_range
        end_byte: 0,   // Not used by lsp_range_from_tree_sitter_range
        start_point: tree_sitter::Point {
            row: line,
            column: 0,
        },
        end_point: tree_sitter::Point {
            row: line,
            column: old_indent_byte,
        },
    };

    let edit = TextEdit {
        range: doc.lsp_range_from_tree_sitter_range(range),
        new_text,
    };

    let mut edits = vec![edit];

    // Indent closing delimiter to mitigate VS Code's indent-outdent behaviour
    // https://github.com/posit-dev/positron/issues/3484
    if bol_parent.is_braced_expression() {
        // FIXME: Use named delim node once available
        let n = bol_parent.child_count();
        if n > 1 {
            let close = bol_parent.child(n - 1).unwrap();
            let close_line = close.start_position().row;

            if close.node_type() == NodeType::Anonymous("}".into()) && close_line > line {
                if let Some(ref mut close_edits) = indent_edit(doc, close_line)? {
                    edits.append(close_edits);
                }
            }
        }
    }

    Ok(Some(edits))
}

fn brace_parent(node: tree_sitter::Node) -> tree_sitter::Node {
    let Some(parent) = node.parent() else {
        return node;
    };

    match parent.node_type() {
        NodeType::FunctionDefinition => parent,
        NodeType::IfStatement => parent,
        NodeType::ForStatement => parent,
        NodeType::WhileStatement => parent,
        NodeType::RepeatStatement => parent,
        _ => node,
    }
}

/// Returns indent as a pair of space size and byte size
pub fn line_indent(text: &str, line: usize, config: &IndentationConfig) -> (usize, usize) {
    let mut byte_indent = 0;
    let mut indent = 0;

    let Some(line_text) = text.lines().nth(line) else {
        return (0, 0);
    };

    for next_char in line_text.chars() {
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

/// Find the nearest node that is a braced expression
pub fn find_enclosing_brace(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    if let Some(parent) = node.parent() {
        parent.ancestors().find(|n| n.is_braced_expression())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use aether_lsp_utils::proto::from_proto;
    use stdext::assert_match;
    use tower_lsp::lsp_types::TextEdit;

    use crate::lsp::config::IndentStyle;
    use crate::lsp::config::IndentationConfig;
    use crate::lsp::documents::Document;
    use crate::lsp::indent::indent_edit;
    use crate::lsp::indent::new_line_indent;

    fn apply_text_edits(edits: Vec<TextEdit>, doc: &mut Document) {
        from_proto::apply_text_edits(
            &mut doc.contents,
            edits,
            &mut doc.line_index,
            doc.position_encoding,
        );
        *doc = test_doc(&doc.contents);
    }

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
    fn test_line_indent_leading_whitespace() {
        // Indent should be unchanged regardless of how much leading whitespace
        // there is before the first newline
        // https://github.com/posit-dev/positron/issues/5258
        let text = String::from("  \nx");
        let doc = test_doc(&text);
        let edit = indent_edit(&doc, 1).unwrap();
        assert!(edit.is_none());

        let text = String::from("\r\nx");
        let doc = test_doc(&text);
        let edit = indent_edit(&doc, 1).unwrap();
        assert!(edit.is_none());
    }

    #[test]
    fn test_line_indent_chains() {
        let mut doc = test_doc("foo +\n  bar +\n    baz + qux |>\nfoofy()");

        // Indenting the first two lines doesn't change the text
        assert_match!(indent_edit(&doc, 0), Ok(None));
        assert_match!(indent_edit(&doc, 1), Ok(None));

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "foo +\n  bar +\n  baz + qux |>\nfoofy()");

        let edit = indent_edit(&doc, 3).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "foo +\n  bar +\n  baz + qux |>\n  foofy()");
    }

    #[test]
    fn test_line_indent_chains_trailing_space() {
        let mut doc = test_doc("foo +\n  bar(\n    x\n  ) +\n    baz\n  ");

        let edit = indent_edit(&doc, 4).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "foo +\n  bar(\n    x\n  ) +\n  baz\n  ");
    }

    #[test]
    fn test_line_indent_chains_outdent() {
        let text = String::from("1 +\n  2\n");
        let doc = test_doc(&text);

        assert_match!(indent_edit(&doc, 2), Ok(None));
    }

    #[test]
    fn test_line_indent_chains_deep() {
        let mut doc = test_doc("deep()()[] +\n    deep()()[]");

        let edit = indent_edit(&doc, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "deep()()[] +\n  deep()()[]");
    }

    #[test]
    fn test_line_indent_chains_deep_newlines() {
        // With newlines in the way
        let mut doc = test_doc("deep(\n)()[] +\ndeep(\n)()[]");

        let edit = indent_edit(&doc, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "deep(\n)()[] +\n  deep(\n)()[]");
    }

    #[test]
    fn test_line_indent_chains_calls() {
        let mut doc = test_doc("foo() +\n  bar() +\nbaz()");

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "foo() +\n  bar() +\n  baz()");

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
        let mut doc = test_doc("{\nbar\n}");

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "{\n  bar\n}");

        let mut doc = test_doc("function() {\nbar\n}");

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "function() {\n  bar\n}");
    }

    #[test]
    fn test_line_indent_braced_expression_closing() {
        let mut doc = test_doc("{\n  }");

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "{\n}");
    }

    #[test]
    fn test_line_indent_braced_expression_closing_multiline() {
        // https://github.com/posit-dev/positron/issues/3484
        let mut doc = test_doc("{\n\n    }");

        let edit = indent_edit(&doc, 1).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "{\n  \n}");
    }

    #[test]
    fn test_line_indent_braced_expression_multiline() {
        let mut doc = test_doc("function(\n        ) {\nfoo\n}");

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "function(\n        ) {\n  foo\n}");
    }

    #[test]
    fn test_line_indent_braced_expression_multiline_empty() {
        let mut doc = test_doc("function(\n        ) {\n\n}");

        let edit = indent_edit(&doc, 2).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "function(\n        ) {\n  \n}");
    }

    #[test]
    fn test_line_indent_minimum() {
        // https://github.com/posit-dev/positron/issues/1683
        let mut doc = test_doc("function() {\n  ({\n  }\n)\n}");

        let edit = indent_edit(&doc, 3).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "function() {\n  ({\n  }\n  )\n}");
    }

    #[test]
    fn test_line_indent_minimum_nested() {
        // Nested R function test with multiple levels of nesting
        let mut doc = test_doc("{\n  {\n    ({\n    }\n  )\n  }\n}");

        let edit = indent_edit(&doc, 4).unwrap().unwrap();
        apply_text_edits(edit, &mut doc);
        assert_eq!(doc.contents, "{\n  {\n    ({\n    }\n    )\n  }\n}");
    }

    #[test]
    fn test_line_indent_function_opening_brace_own_line() {
        let text = String::from("object <- function()\n{\n  body\n}");
        let doc = test_doc(&text);

        assert_match!(indent_edit(&doc, 1).unwrap(), None);
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

        let mut doc = test_doc(&orig);

        let n_lines = doc.contents.matches('\n').count();

        for i in 0..n_lines {
            if let Some(edit) = indent_edit(&doc, i).unwrap() {
                apply_text_edits(edit, &mut doc);
            }
        }

        write_asset("lsp/snapshots/indent.R", &doc.contents);

        if orig != doc.contents {
            panic!("Indentation snapshots have changed.\nPlease see git diff.");
        }
    }
}
