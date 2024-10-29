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

fn new_symbol(name: String, kind: SymbolKind, range: Range) -> DocumentSymbol {
    DocumentSymbol {
        name,
        kind,
        detail: None,
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        range,
        selection_range: range,
    }
}

fn new_symbol_node(
    name: String,
    kind: SymbolKind,
    range: Range,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    let mut symbol = new_symbol(name, kind, range);
    symbol.children = Some(children);
    symbol
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
    store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    Ok(match node.node_type() {
        // Handle comment sections in expression lists
        NodeType::Program | NodeType::BracedExpression => {
            index_expression_list(&node, store, contents)?
        },
        // Index assignments as object or function symbols
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
        NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment) => {
            index_assignment(&node, store, contents)?
        },
        // Nothing to index. FIXME: We should handle argument lists, e.g. to
        // index inside functions passed as arguments, or inside `test_that()`
        // blocks.
        _ => store,
    })
}

// Handles root node and braced lists
fn index_expression_list(
    node: &Node,
    store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    let mut cursor = node.walk();

    // Put level and store in a vector for nested structure handling
    let mut store_stack: Vec<(usize, Vec<DocumentSymbol>)> = vec![(usize::MAX, store)];

    for child in node.children(&mut cursor) {
        match child.node_type() {
            NodeType::Comment => {
                store_stack = index_comments(&child, store_stack, contents)?;
            },
            _ => {
                let (level, store) = store_stack.pop().expect("Stack has always one element");
                let store = index_node(&child, store, contents)?;
                store_stack.push((level, store));
            },
        }
    }

    // Iteratively add the children of the last element of `store_stack` until there is only one element
    while store_stack.len() > 1 {
        store_stack_pop(&mut store_stack);
    }

    // At the end, the remaining element in `store_stack` contains the updated store
    let (_, store) = store_stack.pop().unwrap();
    Ok(store)
}

// Pop store from the stack, adding it as child to its parent (which becomes the
// last element in the stack). Once popped, we no longer need to keep track of level.
fn store_stack_pop(store_stack: &mut Vec<(usize, Vec<DocumentSymbol>)>) {
    // Pop the last element from `store_stack`
    let (last_level, mut last_symbols) = store_stack.pop().unwrap();

    // Add the last_symbols as children to the previous level in `store_stack`
    if let Some((_, parent_symbols)) = store_stack.last_mut() {
        if let Some(parent_symbol) = parent_symbols.last_mut() {
            parent_symbol
                .children
                .as_mut()
                .unwrap()
                .append(&mut last_symbols);
        } else {
            // If there's no last parent symbol, add the last symbols directly
            parent_symbols.append(&mut last_symbols);
        }
    } else {
        // In case there's no parent, just push the `last_symbols` back
        store_stack.push((last_level, last_symbols));
        return;
    }
}

fn index_comments(
    node: &Node,
    mut store_stack: Vec<(usize, Vec<DocumentSymbol>)>,
    contents: &Rope,
) -> anyhow::Result<Vec<(usize, Vec<DocumentSymbol>)>> {
    let comment_text = contents.node_slice(&node)?.to_string();

    // Check if the comment starts with one or more '#' followed by any text and ends with 4+ punctuations
    let Some((level, title)) = parse_comment_as_section(&comment_text) else {
        return Ok(store_stack);
    };

    // Create a symbol based on the parsed comment
    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    let symbol = new_symbol(title, SymbolKind::STRING, Range { start, end });

    // Find the appropriate number of layers to pop from `store_stack`
    let levels: Vec<usize> = store_stack.iter().map(|(level, _)| *level).collect();
    let layer = levels
        .iter()
        .enumerate()
        .rev() // Reverse the iterator to search from right to left
        .find(|&(_, &l)| l < level) // Find the first element that is less than `level`
        .map(|(index, _)| index + 1)
        .unwrap_or(1);

    while store_stack.len() > layer {
        store_stack_pop(&mut store_stack);
    }

    // Add the new symbol to the appropriate level in `store_stack`
    if let Some((last_level, symbols)) = store_stack.last_mut() {
        if *last_level < level {
            // If current level is greater, add a new level to `store_stack`
            store_stack.push((level, vec![symbol]));
        } else if *last_level == level {
            // If current level is equal, push the symbol as a child of the last element
            symbols.push(symbol);
        } else {
            // For handling of the starting nodes, the level of which are set to maximum
            symbols.push(symbol);
            *last_level = level;
        }
    } else {
        // If `store_stack` is empty, add the new symbol at root
        store_stack.push((level, vec![symbol]));
    }

    // Add an empty vector to `store_stack` to subordinate other symbols
    store_stack.push((usize::MAX, vec![])); // usize::MAX to make ensure it is never a parent

    Ok(store_stack)
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

    let symbol = new_symbol(name, SymbolKind::VARIABLE, Range { start, end });
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

    let body = rhs.child_by_field_name("body").into_result()?;

    // At this point we increase the nesting level. Recurse into the function
    // node with a new store of children nodes.
    let children = index_node(&body, vec![], contents)?;

    let mut symbol = new_symbol_node(name, SymbolKind::FUNCTION, range, children);
    symbol.detail = Some(detail);
    store.push(symbol);

    Ok(store)
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
            SymbolKind::VARIABLE,
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

        let mut foo = new_symbol(String::from("foo"), SymbolKind::FUNCTION, range);
        foo.detail = Some(String::from("function()"));

        assert_eq!(test_symbol("foo <- function() {}"), vec![foo]);
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
        let bar = new_symbol(String::from("bar"), SymbolKind::VARIABLE, range);

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
        let mut foo = new_symbol(String::from("foo"), SymbolKind::FUNCTION, range);
        foo.children = Some(vec![bar]);
        foo.detail = Some(String::from("function()"));

        assert_eq!(test_symbol("foo <- function() { bar <- 1 }"), vec![foo]);
    }

    #[test]
    fn test_symbol_assignment_function_nested_section() {
        insta::assert_debug_snapshot!(test_symbol(
            "
## title0 ----
foo <- function() {
  # title1 ----
  ### title2 ----
  ## title3 ----
  # title4 ----
}
# title5 ----"
        ));
    }

    #[test]
    fn test_symbol_braced_list() {
        let range = Range {
            start: Position {
                line: 0,
                character: 2,
            },
            end: Position {
                line: 0,
                character: 5,
            },
        };
        let foo = new_symbol(String::from("foo"), SymbolKind::VARIABLE, range);

        assert_eq!(test_symbol("{ foo <- 1 }"), vec![foo]);
    }
}
