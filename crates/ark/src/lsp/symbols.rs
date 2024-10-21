//
// symbols.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(deprecated)]

use std::result::Result::Ok;

use ropey::Rope;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::DocumentSymbol;
use tower_lsp::lsp_types::DocumentSymbolParams;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::SymbolInformation;
use tower_lsp::lsp_types::SymbolKind;
use tower_lsp::lsp_types::Url;
use tower_lsp::lsp_types::WorkspaceSymbolParams;
use tree_sitter::Node;

use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::indexer;
use crate::lsp::indexer::IndexEntryData;
use crate::lsp::state::WorldState;
use crate::lsp::traits::rope::RopeExt;
use crate::lsp::traits::string::StringExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

fn new_symbol(
    name: String,
    kind: SymbolKind,
    detail: Option<String>,
    range: Range,
) -> DocumentSymbol {
    DocumentSymbol {
        name,
        kind,
        detail,
        // Safety: We assume `children` can't be `None`
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        range,
        selection_range: range,
    }
}

pub fn symbols(params: &WorkspaceSymbolParams) -> anyhow::Result<Vec<SymbolInformation>> {
    let query = &params.query;
    let mut info: Vec<SymbolInformation> = Vec::new();

    indexer::map(|path, symbol, entry| {
        if !symbol.fuzzy_matches(query) {
            return;
        }

        match &entry.data {
            IndexEntryData::Function { name, arguments: _ } => {
                info.push(SymbolInformation {
                    name: name.to_string(),
                    kind: SymbolKind::FUNCTION,
                    location: Location {
                        uri: Url::from_file_path(path).unwrap(),
                        range: entry.range,
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            },

            IndexEntryData::Section { level: _, title } => {
                info.push(SymbolInformation {
                    name: title.to_string(),
                    kind: SymbolKind::STRING,
                    location: Location {
                        uri: Url::from_file_path(path).unwrap(),
                        range: entry.range,
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            },
        };
    });

    Ok(info)
}

pub(crate) fn document_symbols(
    state: &WorldState,
    params: &DocumentSymbolParams,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    let uri = &params.text_document.uri;
    let document = state.documents.get(uri).into_result()?;
    let ast = &document.ast;
    let contents = &document.contents;

    let node = ast.root_node();

    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    // Construct a root symbol, so we always have something to append to
    let range = Range { start, end };
    let root = new_symbol("<root>".to_string(), SymbolKind::NULL, None, range);

    // Index from the root
    let root = match index_node(root, &node, &contents) {
        Ok(root) => root,
        Err(err) => {
            log::error!("Error indexing node: {err:?}");
            return Ok(Vec::new());
        },
    };

    // Return the children we found. Safety: We always set the children to an
    // empty vector.
    Ok(root.children.unwrap())
}

fn is_indexable(node: &Node) -> bool {
    // don't index 'arguments' or 'parameters'
    if matches!(node.node_type(), NodeType::Arguments | NodeType::Parameters) {
        return false;
    }

    true
}

fn push_child(node: &mut DocumentSymbol, child: DocumentSymbol) {
    // Safety: The LSP protocol wraps the list of children in an option but we
    // always set it to an empty vector.
    node.children.as_mut().unwrap().push(child);
}

// Function to parse a comment and return the section level and title
fn parse_comment_as_section(comment: &str) -> Option<(usize, String)> {
    // Match lines starting with one or more '#' followed by some non-empty content and must end with 4 or more '-', '#', or `=`
    // Ensure that there's actual content between the start and the trailing symbols.
    if let Some(caps) = indexer::RE_COMMENT_SECTION.captures(comment) {
        let hashes = caps.get(1)?.as_str().len(); // Count the number of '#'
        let title = caps.get(2)?.as_str().trim().to_string(); // Extract the title text without trailing punctuations
        if title.is_empty() {
            return None; // Return None for lines with only hashtags
        }
        return Some((hashes, title)); // Return the level based on the number of '#' and the title
    }

    None
}

fn index_node(
    mut parent: DocumentSymbol,
    node: &Node,
    contents: &Rope,
) -> anyhow::Result<DocumentSymbol> {
    // Check if the node is a comment and matches the markdown-style comment patterns
    if node.node_type() == NodeType::Comment {
        let comment_text = contents.node_slice(&node)?.to_string();

        // Check if the comment starts with one or more '#' followed by any text and ends with 4+ punctuations
        if let Some((_level, title)) = parse_comment_as_section(&comment_text) {
            // Create a symbol based on the parsed comment
            let start = convert_point_to_position(contents, node.start_position());
            let end = convert_point_to_position(contents, node.end_position());

            let symbol = new_symbol(title, SymbolKind::STRING, None, Range { start, end });
            push_child(&mut parent, symbol);

            // Return early to avoid further processing
            return Ok(parent);
        }
    }

    if matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    ) {
        parent = index_assignment(parent, node, contents)?;
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_indexable(&child) {
            parent = index_node(parent, &child, contents)?;
        }
    }

    Ok(parent)
}

fn index_assignment(
    mut parent: DocumentSymbol,
    node: &Node,
    contents: &Rope,
) -> anyhow::Result<DocumentSymbol> {
    // check for assignment
    matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    )
    .into_result()?;

    // check for lhs, rhs
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    // check for identifier on lhs, function on rhs
    let function = lhs.is_identifier_or_string() && rhs.is_function_definition();

    if function {
        return index_assignment_with_function(parent, node, contents);
    }

    // otherwise, just index as generic object
    let name = contents.node_slice(&lhs)?.to_string();

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    let symbol = new_symbol(name, SymbolKind::VARIABLE, None, Range { start, end });
    push_child(&mut parent, symbol);

    Ok(parent)
}

fn index_assignment_with_function(
    mut parent: DocumentSymbol,
    node: &Node,
    contents: &Rope,
) -> anyhow::Result<DocumentSymbol> {
    // check for lhs, rhs
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    // start extracting the argument names
    let mut arguments: Vec<String> = Vec::new();
    let parameters = rhs.child_by_field_name("parameters").into_result()?;

    let mut cursor = parameters.walk();
    for parameter in parameters.children_by_field_name("parameter", &mut cursor) {
        let name = parameter.child_by_field_name("name").into_result()?;
        let name = contents.node_slice(&name)?.to_string();
        arguments.push(name);
    }

    let name = contents.node_slice(&lhs)?.to_string();
    let detail = format!("function({})", arguments.join(", "));

    let range = Range {
        start: convert_point_to_position(contents, lhs.start_position()),
        end: convert_point_to_position(contents, rhs.end_position()),
    };
    let symbol = new_symbol(name, SymbolKind::FUNCTION, Some(detail), range);

    // Recurse into the function node
    let symbol = index_node(symbol, &rhs, contents)?;

    // Set as child after recursing, now that we own the symbol again
    push_child(&mut parent, symbol);
    Ok(parent)
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Position;

    use super::*;
    use crate::lsp::documents::Document;

    fn test_symbol(code: &str) -> Vec<DocumentSymbol> {
        let doc = Document::new(code, None);
        let node = doc.ast.root_node();

        let start = convert_point_to_position(&doc.contents, node.start_position());
        let end = convert_point_to_position(&doc.contents, node.end_position());

        let root = DocumentSymbol {
            name: String::from("<root>"),
            kind: SymbolKind::NULL,
            children: Some(Vec::new()),
            deprecated: None,
            tags: None,
            detail: None,
            range: Range { start, end },
            selection_range: Range { start, end },
        };

        let root = index_node(root, &node, &doc.contents).unwrap();
        root.children.unwrap_or_default()
    }

    #[test]
    fn test_symbol_parse_comment_as_section() {
        assert_eq!(parse_comment_as_section("# foo"), None);
        assert_eq!(parse_comment_as_section("# foo ---"), None);
        assert_eq!(parse_comment_as_section("########"), None);
        assert_eq!(
            parse_comment_as_section("# foo ----"),
            Some((1, String::from("foo")))
        );
    }

    #[test]
    fn test_symbol_comment_sections() {
        assert_eq!(test_symbol("# foo"), vec![]);
        assert_eq!(test_symbol("# foo ---"), vec![]);
        assert_eq!(test_symbol("########"), vec![]);

        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 10,
            },
        };
        assert_eq!(test_symbol("# foo ----"), vec![DocumentSymbol {
            name: String::from("foo"),
            kind: SymbolKind::STRING,
            children: Some(Vec::new()),
            deprecated: None,
            tags: None,
            detail: None,
            range,
            selection_range: range,
        }]);
    }
}
