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

fn new_symbol_node(
    name: String,
    kind: SymbolKind,
    detail: Option<String>,
    range: Range,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    let mut symbol = new_symbol(name, kind, detail, range);
    symbol.children = Some(children);
    symbol
}

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

    // Index from the root
    match index_node(&node, vec![], &contents) {
        Ok(children) => Ok(children),
        Err(err) => {
            log::error!("Error indexing node: {err:?}");
            return Ok(Vec::new());
        },
    }
}

fn index_node(
    node: &Node,
    mut store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    // Check if the node is a comment and matches the markdown-style comment patterns
    if node.node_type() == NodeType::Comment {
        let comment_text = contents.node_slice(&node)?.to_string();

        // Check if the comment starts with one or more '#' followed by any text and ends with 4+ punctuations
        if let Some((_level, title)) = parse_comment_as_section(&comment_text) {
            // Create a symbol based on the parsed comment
            let start = convert_point_to_position(contents, node.start_position());
            let end = convert_point_to_position(contents, node.end_position());

            let symbol = new_symbol(title, SymbolKind::STRING, None, Range { start, end });
            store.push(symbol);

            // Return early to avoid further processing
            return Ok(store);
        }
    }

    if matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    ) {
        return index_assignment(node, store, contents);
    }

    // Recurse into children. We're in the same outline section so use the same store.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_indexable(&child) {
            store = index_node(&child, store, contents)?;
        }
    }

    Ok(store)
}

fn index_assignment(
    node: &Node,
    mut store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    // Check for assignment
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
        return index_assignment_with_function(node, store, contents);
    }

    // otherwise, just index as generic object
    let name = contents.node_slice(&lhs)?.to_string();

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    let symbol = new_symbol(name, SymbolKind::VARIABLE, None, Range { start, end });
    store.push(symbol);

    Ok(store)
}

fn index_assignment_with_function(
    node: &Node,
    mut store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
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

    // At this point we increase the nesting level. Recurse into the function
    // node with a new store of children nodes.
    let children = index_node(&rhs, vec![], contents)?;

    let symbol = new_symbol_node(name, SymbolKind::FUNCTION, Some(detail), range, children);
    store.push(symbol);

    Ok(store)
}

fn is_indexable(node: &Node) -> bool {
    // Don't index 'arguments' or 'parameters'
    if matches!(node.node_type(), NodeType::Arguments | NodeType::Parameters) {
        return false;
    }

    true
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

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Position;

    use super::*;
    use crate::lsp::documents::Document;

    fn test_symbol(code: &str) -> Vec<DocumentSymbol> {
        let doc = Document::new(code, None);
        let node = doc.ast.root_node();

        index_node(&node, vec![], &doc.contents).unwrap()
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
        assert_eq!(test_symbol("# foo ----"), vec![new_symbol(
            String::from("foo"),
            SymbolKind::STRING,
            None,
            range
        )]);
    }

    #[test]
    fn test_symbol_assignment() {
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 3,
            },
        };
        assert_eq!(test_symbol("foo <- 1"), vec![new_symbol(
            String::from("foo"),
            SymbolKind::OBJECT,
            None,
            range,
        )]);
    }

    #[test]
    fn test_symbol_assignment_function() {
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 20,
            },
        };
        assert_eq!(test_symbol("foo <- function() {}"), vec![new_symbol(
            String::from("foo"),
            SymbolKind::FUNCTION,
            Some(String::from("function()")),
            range,
        )]);
    }

    #[test]
    fn test_symbol_assignment_function_nested() {
        let range = Range {
            start: Position {
                line: 0,
                character: 20,
            },
            end: Position {
                line: 0,
                character: 23,
            },
        };
        let bar = new_symbol(String::from("bar"), SymbolKind::OBJECT, None, range);

        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 30,
            },
        };
        let mut foo = new_symbol(
            String::from("foo"),
            SymbolKind::FUNCTION,
            Some(String::from("function()")),
            range,
        );
        foo.children = Some(vec![bar]);

        assert_eq!(test_symbol("foo <- function() { bar <- 1 }"), vec![foo]);
    }
}
