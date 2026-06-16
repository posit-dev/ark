use anyhow::anyhow;
use oak_db::File;

use crate::lsp::backend::LspError;
use crate::lsp::backend::LspResult;
use crate::lsp::config::IndentStyle;
use crate::lsp::config::IndentationConfig;
use crate::lsp::db::ArkDb;
use crate::lsp::db::FileArkExt;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

/// An indentation correction in tree-sitter coordinates.
pub(crate) struct IndentEdit {
    pub(crate) range: tree_sitter::Range,
    pub(crate) new_text: String,
}

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
pub(crate) fn indent_edit(
    db: &dyn ArkDb,
    file: File,
    config: &IndentationConfig,
    line: usize,
) -> LspResult<Option<Vec<IndentEdit>>> {
    let text = file.source_text(db).as_str();
    let ast = file.tree_sitter(db);

    let line_count = if text.is_empty() {
        1
    } else {
        // Note that `lines().count()` doesn't count trailing newlines
        text.chars().filter(|c| *c == '\n').count() + 1
    };
    if line >= line_count {
        return Err(LspError::Anyhow(anyhow!("`line` is OOB")));
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
                .find(|n| n.parent().is_none_or(|p| !p.is_binary_operator()))
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

    let edit = IndentEdit { range, new_text };

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
                if let Some(ref mut close_edits) = indent_edit(db, file, config, close_line)? {
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
            indent += 1;
            byte_indent += 1;
            continue;
        } else if next_char == '\t' {
            indent += config.tab_width;
            byte_indent += 1;
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
    use aether_lsp_utils::proto::PositionEncoding;
    use biome_line_index::WideEncoding;
    use oak_db::OakDatabase;
    use salsa::Setter;
    use stdext::assert_match;
    use tower_lsp::lsp_types::TextEdit;

    use crate::lsp::ark_file::lsp_range_from_tree_sitter_range;
    use crate::lsp::ark_file::ArkFile;
    use crate::lsp::config::IndentStyle;
    use crate::lsp::config::IndentationConfig;
    use crate::lsp::indent::indent_edit;
    use crate::lsp::indent::new_line_indent;
    use crate::lsp::indent::IndentEdit;

    // NOTE: If we keep adding tests we might want to switch to snapshot tests

    const SPACE_CFG: IndentationConfig = IndentationConfig {
        indent_style: IndentStyle::Space,
        indent_size: 2,
        tab_width: 2,
    };

    const ENCODING: PositionEncoding = PositionEncoding::Wide(WideEncoding::Utf16);

    fn apply_text_edits(
        edits: Vec<IndentEdit>,
        db: &mut OakDatabase,
        file: &ArkFile,
        encoding: PositionEncoding,
    ) {
        let line_index = file.line_index(&*db).clone();
        let edits = edits
            .into_iter()
            .map(|edit| TextEdit {
                range: lsp_range_from_tree_sitter_range(edit.range, &line_index, encoding).unwrap(),
                new_text: edit.new_text,
            })
            .collect();

        let mut contents = file.contents(&*db).to_string();
        let mut line_index = line_index;
        from_proto::apply_text_edits(&mut contents, edits, &mut line_index, encoding);
        file.file.set_source_text_override(db).to(Some(contents));
    }

    #[test]
    fn test_line_indent_oob() {
        let (db, ark_file) = crate::lsp::ark_file::test_ark_file("");
        assert_match!(
            indent_edit(&db, ark_file.file, &ark_file.config.indent, 1),
            Err(_)
        );

        let (db, ark_file) = crate::lsp::ark_file::test_ark_file("\n");
        assert_match!(
            indent_edit(&db, ark_file.file, &ark_file.config.indent, 2),
            Err(_)
        );
    }

    #[test]
    fn test_line_indent_leading_whitespace() {
        // Indent should be unchanged regardless of how much leading whitespace
        // there is before the first newline
        // https://github.com/posit-dev/positron/issues/5258
        let text = String::from("  \nx");
        let (db, file) = crate::lsp::ark_file::test_ark_file(&text);
        let edit = indent_edit(&db, file.file, &file.config.indent, 1).unwrap();
        assert!(edit.is_none());

        let text = String::from("\r\nx");
        let (db, file) = crate::lsp::ark_file::test_ark_file(&text);
        let edit = indent_edit(&db, file.file, &file.config.indent, 1).unwrap();
        assert!(edit.is_none());
    }

    #[test]
    fn test_line_indent_chains() {
        let (mut db, file) =
            crate::lsp::ark_file::test_ark_file("foo +\n  bar +\n    baz + qux |>\nfoofy()");

        // Indenting the first two lines doesn't change the text
        assert_match!(
            indent_edit(&db, file.file, &file.config.indent, 0),
            Ok(None)
        );
        assert_match!(
            indent_edit(&db, file.file, &file.config.indent, 1),
            Ok(None)
        );

        let edit = indent_edit(&db, file.file, &file.config.indent, 2)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(
            file.contents(&db),
            "foo +\n  bar +\n  baz + qux |>\nfoofy()"
        );

        let edit = indent_edit(&db, file.file, &file.config.indent, 3)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(
            file.contents(&db),
            "foo +\n  bar +\n  baz + qux |>\n  foofy()"
        );
    }

    #[test]
    fn test_line_indent_chains_trailing_space() {
        let (mut db, file) =
            crate::lsp::ark_file::test_ark_file("foo +\n  bar(\n    x\n  ) +\n    baz\n  ");

        let edit = indent_edit(&db, file.file, &file.config.indent, 4)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "foo +\n  bar(\n    x\n  ) +\n  baz\n  ");
    }

    #[test]
    fn test_line_indent_chains_outdent() {
        let text = String::from("1 +\n  2\n");
        let (db, file) = crate::lsp::ark_file::test_ark_file(&text);

        assert_match!(
            indent_edit(&db, file.file, &file.config.indent, 2),
            Ok(None)
        );
    }

    #[test]
    fn test_line_indent_chains_deep() {
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("deep()()[] +\n    deep()()[]");

        let edit = indent_edit(&db, file.file, &file.config.indent, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&db, file.file, &file.config.indent, 1)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "deep()()[] +\n  deep()()[]");
    }

    #[test]
    fn test_line_indent_chains_deep_newlines() {
        // With newlines in the way
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("deep(\n)()[] +\ndeep(\n)()[]");

        let edit = indent_edit(&db, file.file, &file.config.indent, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&db, file.file, &file.config.indent, 2)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "deep(\n)()[] +\n  deep(\n)()[]");
    }

    #[test]
    fn test_line_indent_chains_calls() {
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("foo() +\n  bar() +\nbaz()");

        let edit = indent_edit(&db, file.file, &file.config.indent, 2)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "foo() +\n  bar() +\n  baz()");

        // Indenting the first two lines doesn't change the text
        let edit = indent_edit(&db, file.file, &file.config.indent, 0).unwrap();
        assert!(edit.is_none());

        let edit = indent_edit(&db, file.file, &file.config.indent, 1).unwrap();
        assert!(edit.is_none());

        let (db, file) = crate::lsp::ark_file::test_ark_file("foo(\n) +\n  bar");
        let edit = indent_edit(&db, file.file, &file.config.indent, 0).unwrap();
        assert!(edit.is_none());
    }

    #[test]
    fn test_line_indent_braced_expression() {
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("{\nbar\n}");

        let edit = indent_edit(&db, file.file, &file.config.indent, 1)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "{\n  bar\n}");

        let (mut db, ark_file) = crate::lsp::ark_file::test_ark_file("function() {\nbar\n}");

        let edit = indent_edit(&db, ark_file.file, &ark_file.config.indent, 1)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &ark_file, ENCODING);
        assert_eq!(ark_file.contents(&db), "function() {\n  bar\n}");
    }

    #[test]
    fn test_line_indent_braced_expression_closing() {
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("{\n  }");

        let edit = indent_edit(&db, file.file, &file.config.indent, 1)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "{\n}");
    }

    #[test]
    fn test_line_indent_braced_expression_closing_multiline() {
        // https://github.com/posit-dev/positron/issues/3484
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("{\n\n    }");

        let edit = indent_edit(&db, file.file, &file.config.indent, 1)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "{\n  \n}");
    }

    #[test]
    fn test_line_indent_braced_expression_multiline() {
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("function(\n        ) {\nfoo\n}");

        let edit = indent_edit(&db, file.file, &file.config.indent, 2)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "function(\n        ) {\n  foo\n}");
    }

    #[test]
    fn test_line_indent_braced_expression_multiline_empty() {
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("function(\n        ) {\n\n}");

        let edit = indent_edit(&db, file.file, &file.config.indent, 2)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "function(\n        ) {\n  \n}");
    }

    #[test]
    fn test_line_indent_minimum() {
        // https://github.com/posit-dev/positron/issues/1683
        let (mut db, file) = crate::lsp::ark_file::test_ark_file("function() {\n  ({\n  }\n)\n}");

        let edit = indent_edit(&db, file.file, &file.config.indent, 3)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "function() {\n  ({\n  }\n  )\n}");
    }

    #[test]
    fn test_line_indent_minimum_nested() {
        // Nested R function test with multiple levels of nesting
        let (mut db, file) =
            crate::lsp::ark_file::test_ark_file("{\n  {\n    ({\n    }\n  )\n  }\n}");

        let edit = indent_edit(&db, file.file, &file.config.indent, 4)
            .unwrap()
            .unwrap();
        apply_text_edits(edit, &mut db, &file, ENCODING);
        assert_eq!(file.contents(&db), "{\n  {\n    ({\n    }\n    )\n  }\n}");
    }

    #[test]
    fn test_line_indent_function_opening_brace_own_line() {
        let text = String::from("object <- function()\n{\n  body\n}");
        let (db, file) = crate::lsp::ark_file::test_ark_file(&text);

        assert_match!(
            indent_edit(&db, file.file, &file.config.indent, 1).unwrap(),
            None
        );
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
        let (mut db, file) = crate::lsp::ark_file::test_ark_file(&orig);
        let n_lines = file.contents(&db).matches('\n').count();
        for i in 0..n_lines {
            if let Some(edit) = indent_edit(&db, file.file, &file.config.indent, i).unwrap() {
                apply_text_edits(edit, &mut db, &file, ENCODING);
            }
        }
        let result = file.contents(&db).to_string();
        write_asset("lsp/snapshots/indent.R", &result);
        if orig != result {
            panic!("Indentation snapshots have changed.\nPlease see git diff.");
        }
    }
}
