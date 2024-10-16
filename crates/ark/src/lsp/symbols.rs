//
// symbols.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(deprecated)]

use std::result::Result::Ok;

use anyhow::*;
use log::*;
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
    let mut symbols: Vec<DocumentSymbol> = Vec::new();

    let uri = &params.text_document.uri;
    let document = state.documents.get(uri).into_result()?;
    let ast = &document.ast;
    let contents = &document.contents;

    let node = ast.root_node();

    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    // construct a root symbol, so we always have something to append to
    let mut root = DocumentSymbol {
        name: "<root>".to_string(),
        kind: SymbolKind::NULL,
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        detail: None,
        range: Range { start, end },
        selection_range: Range { start, end },
    };

    // index from the root
    index_node(&node, &contents, &mut root, &mut symbols)?;

    // return the children we found
    Ok(root.children.unwrap_or_default())
}

fn is_indexable(node: &Node) -> bool {
    // don't index 'arguments' or 'parameters'
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

fn index_node(
    node: &Node,
    contents: &Rope,
    parent: &mut DocumentSymbol,
    symbols: &mut Vec<DocumentSymbol>,
) -> Result<bool> {
    // Maintain a stack to track the hierarchy of comment sections
    let mut section_stack: Vec<(usize, *mut DocumentSymbol)> =
        vec![(0, parent as *mut DocumentSymbol)];

    // Check if the node is a comment and matches the markdown-style comment patterns
    if node.node_type() == NodeType::Comment {
        let comment_text = contents.node_slice(&node)?.to_string();

        // Check if the comment starts with one or more '#' followed by any text and ends with 4+ punctuations
        if let Some((level, title)) = parse_comment_as_section(&comment_text) {
            // Create a symbol based on the parsed comment
            let start = convert_point_to_position(contents, node.start_position());
            let end = convert_point_to_position(contents, node.end_position());

            let symbol = DocumentSymbol {
                name: title,
                kind: SymbolKind::STRING, // Treat it as a string section
                detail: None,
                children: Some(Vec::new()), // Prepare for child symbols if any
                deprecated: None,
                tags: None,
                range: Range { start, end },
                selection_range: Range { start, end },
            };

            // Pop the stack until we find the appropriate parent level
            while let Some((current_level, _)) = section_stack.last() {
                if *current_level >= level {
                    section_stack.pop();
                } else {
                    break;
                }
            }

            // Get a mutable reference to the current parent from the stack
            if let Some((_, current_parent_ptr)) = section_stack.last() {
                // SAFETY: We know that the pointer is still valid because it's derived from the current stack state
                let current_parent = unsafe { &mut **current_parent_ptr };

                current_parent.children.as_mut().unwrap().push(symbol);
                let new_parent = current_parent
                    .children
                    .as_mut()
                    .unwrap()
                    .last_mut()
                    .unwrap();
                section_stack.push((level, new_parent as *mut DocumentSymbol));
            } else {
                // If no correct parent is found, add this as a root-level comment
                parent.children.as_mut().unwrap().push(symbol);
                let new_parent = parent.children.as_mut().unwrap().last_mut().unwrap();
                section_stack.push((level, new_parent as *mut DocumentSymbol));
            }

            // Return early to avoid further processing of the current node
            return Ok(true);
        }
    }

    if matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    ) {
        // Use the last entry in the stack to determine the correct parent
        if let Some((_, current_parent_ptr)) = section_stack.last() {
            let current_parent = unsafe { &mut **current_parent_ptr };
            match index_assignment(node, contents, current_parent, symbols) {
                Ok(handled) => {
                    if handled {
                        return Ok(true);
                    }
                },
                Err(error) => error!("{:?}", error),
            }
        }
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_indexable(&child) {
            // Use the last entry in the stack to determine the correct parent
            if let Some((_, current_parent_ptr)) = section_stack.last() {
                let current_parent = unsafe { &mut **current_parent_ptr };
                let result = index_node(&child, contents, current_parent, symbols);
                if let Err(error) = result {
                    error!("{:?}", error);
                }
            }
        }
    }

    Ok(true)
}

fn index_assignment(
    node: &Node,
    contents: &Rope,
    parent: &mut DocumentSymbol,
    symbols: &mut Vec<DocumentSymbol>,
) -> Result<bool> {
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
        return index_assignment_with_function(node, contents, parent, symbols);
    }

    // otherwise, just index as generic object
    let name = contents.node_slice(&lhs)?.to_string();

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    let symbol = DocumentSymbol {
        name,
        kind: SymbolKind::OBJECT,
        detail: None,
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        range: Range::new(start, end),
        selection_range: Range::new(start, end),
    };

    // add this symbol to the parent node
    parent.children.as_mut().unwrap().push(symbol);

    Ok(true)
}

fn index_assignment_with_function(
    node: &Node,
    contents: &Rope,
    parent: &mut DocumentSymbol,
    symbols: &mut Vec<DocumentSymbol>,
) -> Result<bool> {
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

    // build the document symbol
    let symbol = DocumentSymbol {
        name,
        kind: SymbolKind::FUNCTION,
        detail: Some(detail),
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        range: Range {
            start: convert_point_to_position(contents, lhs.start_position()),
            end: convert_point_to_position(contents, rhs.end_position()),
        },
        selection_range: Range {
            start: convert_point_to_position(contents, lhs.start_position()),
            end: convert_point_to_position(contents, lhs.end_position()),
        },
    };

    // add this symbol to the parent node
    parent.children.as_mut().unwrap().push(symbol);

    // recurse into this node
    let parent = parent.children.as_mut().unwrap().last_mut().unwrap();
    index_node(&rhs, contents, parent, symbols)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Position;

    use super::*;
    use crate::lsp::documents::Document;

    fn test_symbol(code: &str) -> Vec<DocumentSymbol> {
        let mut symbols: Vec<DocumentSymbol> = Vec::new();

        let doc = Document::new(code, None);
        let node = doc.ast.root_node();

        let start = convert_point_to_position(&doc.contents, node.start_position());
        let end = convert_point_to_position(&doc.contents, node.end_position());

        let mut root = DocumentSymbol {
            name: String::from("<root>"),
            kind: SymbolKind::NULL,
            children: Some(Vec::new()),
            deprecated: None,
            tags: None,
            detail: None,
            range: Range { start, end },
            selection_range: Range { start, end },
        };

        index_node(&node, &doc.contents, &mut root, &mut symbols).unwrap();
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
